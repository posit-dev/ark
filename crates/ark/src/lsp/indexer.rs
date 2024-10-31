//
// indexer.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::collections::HashMap;
use std::path::Path;
use std::result::Result::Ok;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;

use anyhow::anyhow;
use regex::Regex;
use ropey::Rope;
use stdext::unwrap;
use stdext::unwrap::IntoResult;
use tower_lsp::lsp_types::Range;
use tree_sitter::Node;
use walkdir::DirEntry;
use walkdir::WalkDir;

use crate::lsp;
use crate::lsp::documents::Document;
use crate::lsp::encoding::convert_point_to_position;
use crate::lsp::traits::rope::RopeExt;
use crate::treesitter::BinaryOperatorType;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

#[derive(Clone, Debug)]
pub enum IndexEntryData {
    Variable {
        name: String,
    },
    Function {
        name: String,
        arguments: Vec<String>,
    },
    Section {
        level: usize,
        title: String,
    },
}

#[derive(Clone, Debug)]
pub struct IndexEntry {
    pub key: String,
    pub range: Range,
    pub data: IndexEntryData,
}

type DocumentPath = String;
type DocumentSymbol = String;
type DocumentSymbolIndex = HashMap<DocumentSymbol, IndexEntry>;
type WorkspaceIndex = Arc<Mutex<HashMap<DocumentPath, DocumentSymbolIndex>>>;

static WORKSPACE_INDEX: LazyLock<WorkspaceIndex> = LazyLock::new(|| Default::default());
pub static RE_COMMENT_SECTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(#+)\s*(.*?)\s*[#=-]{4,}\s*$").unwrap());

#[tracing::instrument(level = "info", skip_all)]
pub fn start(folders: Vec<String>) {
    let now = std::time::Instant::now();
    lsp::log_info!("Initial indexing started");

    for folder in folders {
        let walker = WalkDir::new(folder);
        for entry in walker.into_iter().filter_entry(|e| filter_entry(e)) {
            if let Ok(entry) = entry {
                if entry.file_type().is_file() {
                    if let Err(err) = index_file(entry.path()) {
                        lsp::log_error!("Can't index file {:?}: {err:?}", entry.path());
                    }
                }
            }
        }
    }

    lsp::log_info!(
        "Initial indexing finished after {}ms",
        now.elapsed().as_millis()
    );
}

pub fn find(symbol: &str) -> Option<(String, IndexEntry)> {
    let index = WORKSPACE_INDEX.lock().unwrap();

    for (path, index) in index.iter() {
        if let Some(entry) = index.get(symbol) {
            return Some((path.clone(), entry.clone()));
        }
    }

    None
}

pub fn map(mut callback: impl FnMut(&Path, &String, &IndexEntry)) {
    let index = WORKSPACE_INDEX.lock().unwrap();

    for (path, index) in index.iter() {
        for (symbol, entry) in index.iter() {
            let path = Path::new(path);
            callback(path, symbol, entry);
        }
    }
}

#[tracing::instrument(level = "trace", skip_all, fields(path = ?path))]
pub fn update(document: &Document, path: &Path) -> anyhow::Result<()> {
    clear(path)?;
    index_document(document, path);
    Ok(())
}

fn insert(path: &Path, entry: IndexEntry) -> anyhow::Result<()> {
    let mut index = WORKSPACE_INDEX.lock().unwrap();
    let path = str_from_path(path)?;

    let index = index.entry(path.to_string()).or_default();
    index.insert(entry.key.clone(), entry);

    Ok(())
}

fn clear(path: &Path) -> anyhow::Result<()> {
    let mut index = WORKSPACE_INDEX.lock().unwrap();
    let path = str_from_path(path)?;

    // Only clears if the `path` was an existing key
    index.entry(path.into()).and_modify(|index| {
        index.clear();
    });

    Ok(())
}

fn str_from_path(path: &Path) -> anyhow::Result<&str> {
    path.to_str().ok_or(anyhow!(
        "Couldn't convert path {} to string",
        path.display()
    ))
}

// TODO: Should we consult the project .gitignore for ignored files?
// TODO: What about front-end ignores?
// TODO: What about other kinds of ignores (e.g. revdepcheck)?
pub fn filter_entry(entry: &DirEntry) -> bool {
    let name = entry.file_name();

    // skip common ignores
    for ignore in [".git", ".Rproj.user", "node_modules", "revdep"] {
        if name == ignore {
            return false;
        }
    }

    // skip project 'renv' folder
    if name == "renv" {
        let companion = entry.path().join("activate.R");
        if companion.exists() {
            return false;
        }
    }

    true
}

fn index_file(path: &Path) -> anyhow::Result<()> {
    // only index R files
    let ext = path.extension().unwrap_or_default();
    if ext != "r" && ext != "R" {
        return Ok(());
    }

    // TODO: Handle document encodings here.
    // TODO: Check if there's an up-to-date buffer to be used.
    let contents = std::fs::read(path)?;
    let contents = String::from_utf8(contents)?;
    let document = Document::new(contents.as_str(), None);

    index_document(&document, path);

    Ok(())
}

fn index_document(document: &Document, path: &Path) {
    let ast = &document.ast;
    let contents = &document.contents;

    let root = ast.root_node();
    let mut cursor = root.walk();
    for node in root.children(&mut cursor) {
        if let Err(err) = match index_node(path, contents, &node) {
            Ok(Some(entry)) => insert(path, entry),
            Ok(None) => Ok(()),
            Err(err) => Err(err),
        } {
            lsp::log_error!("Can't index document: {err:?}");
        }
    }
}

fn index_node(path: &Path, contents: &Rope, node: &Node) -> anyhow::Result<Option<IndexEntry>> {
    if let Ok(Some(entry)) = index_function(path, contents, node) {
        return Ok(Some(entry));
    }

    // Should be after function indexing as this is a more general case
    if let Ok(Some(entry)) = index_variable(path, contents, node) {
        return Ok(Some(entry));
    }

    if let Ok(Some(entry)) = index_comment(path, contents, node) {
        return Ok(Some(entry));
    }

    Ok(None)
}

fn index_function(
    _path: &Path,
    contents: &Rope,
    node: &Node,
) -> anyhow::Result<Option<IndexEntry>> {
    // Check for assignment.
    matches!(
        node.node_type(),
        NodeType::BinaryOperator(BinaryOperatorType::LeftAssignment) |
            NodeType::BinaryOperator(BinaryOperatorType::EqualsAssignment)
    )
    .into_result()?;

    // Check for identifier on left-hand side.
    let lhs = node.child_by_field_name("lhs").into_result()?;
    lhs.is_identifier_or_string().into_result()?;

    // Check for a function definition on the right-hand side.
    let rhs = node.child_by_field_name("rhs").into_result()?;
    rhs.is_function_definition().into_result()?;

    let name = contents.node_slice(&lhs)?.to_string();
    let mut arguments = Vec::new();

    // Get the parameters node.
    let parameters = rhs.child_by_field_name("parameters").into_result()?;

    // Iterate through each, and get the names.
    let mut cursor = parameters.walk();
    for child in parameters.children(&mut cursor) {
        let name = unwrap!(child.child_by_field_name("name"), None => continue);
        if name.is_identifier() {
            let name = contents.node_slice(&name)?.to_string();
            arguments.push(name);
        }
    }

    let start = convert_point_to_position(contents, lhs.start_position());
    let end = convert_point_to_position(contents, lhs.end_position());

    Ok(Some(IndexEntry {
        key: name.clone(),
        range: Range { start, end },
        data: IndexEntryData::Function {
            name: name.clone(),
            arguments,
        },
    }))
}

fn index_variable(
    _path: &Path,
    contents: &Rope,
    node: &Node,
) -> anyhow::Result<Option<IndexEntry>> {
    if !matches!(
        node.node_type(),
        NodeType::BinaryOperator(BinaryOperatorType::LeftAssignment) |
            NodeType::BinaryOperator(BinaryOperatorType::EqualsAssignment)
    ) {
        return Ok(None);
    }

    let Some(lhs) = node.child_by_field_name("lhs") else {
        return Ok(None);
    };
    if !lhs.is_identifier_or_string() {
        return Ok(None);
    }
    let name = contents.node_slice(&lhs)?.to_string();

    let start = convert_point_to_position(contents, lhs.start_position());
    let end = convert_point_to_position(contents, lhs.end_position());

    Ok(Some(IndexEntry {
        key: name.clone(),
        range: Range { start, end },
        data: IndexEntryData::Variable { name },
    }))
}

fn index_comment(_path: &Path, contents: &Rope, node: &Node) -> anyhow::Result<Option<IndexEntry>> {
    // check for comment
    node.is_comment().into_result()?;

    // see if it looks like a section
    let comment = contents.node_slice(node)?.to_string();
    let matches = RE_COMMENT_SECTION
        .captures(comment.as_str())
        .into_result()?;

    let level = matches.get(1).into_result()?;
    let title = matches.get(2).into_result()?;

    let level = level.as_str().len();
    let title = title.as_str().to_string();

    // skip things that look like knitr output
    if title.starts_with("----") {
        return Ok(None);
    }

    let start = convert_point_to_position(contents, node.start_position());
    let end = convert_point_to_position(contents, node.end_position());

    Ok(Some(IndexEntry {
        key: title.clone(),
        range: Range::new(start, end),
        data: IndexEntryData::Section { level, title },
    }))
}
