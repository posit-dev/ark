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
use std::sync::Mutex;
use std::time::SystemTime;

use anyhow::*;
use lazy_static::lazy_static;
use log::*;
use regex::Regex;
use ropey::Rope;
use stdext::unwrap;
use stdext::unwrap::IntoResult;
use tower_lsp::lsp_types::Range;
use tree_sitter::Node;
use walkdir::DirEntry;
use walkdir::WalkDir;

use crate::lsp::documents::Document;
use crate::lsp::encoding::convert_point_to_position;
use crate::lsp::traits::rope::RopeExt;

#[derive(Clone, Debug)]
pub enum IndexEntryData {
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

lazy_static! {
    static ref WORKSPACE_INDEX: WorkspaceIndex = Default::default();
    static ref RE_COMMENT_SECTION: Regex = Regex::new(r"^\s*(#+)\s*(.*?)\s*[#=-]{4,}\s*$").unwrap();
}

pub fn start(folders: Vec<String>) {
    // create a task that indexes these folders
    let _handle = tokio::spawn(async move {
        let now = SystemTime::now();
        info!("Indexing started.");

        for folder in folders {
            let walker = WalkDir::new(folder);
            for entry in walker.into_iter().filter_entry(|e| filter_entry(e)) {
                if let Ok(entry) = entry {
                    if entry.file_type().is_file() {
                        if let Err(error) = index_file(entry.path()) {
                            error!("{:?}", error);
                        }
                    }
                }
            }
        }

        if let Ok(elapsed) = now.elapsed() {
            info!("Indexing finished after {:?}.", elapsed);
        }
    });
}

pub fn find(symbol: &str) -> Option<(String, IndexEntry)> {
    // get index lock
    let index = unwrap!(WORKSPACE_INDEX.lock(), Err(error) => {
        error!("{:?}", error);
        return None;
    });

    // start iterating through index entries
    for (path, index) in index.iter() {
        if let Some(entry) = index.get(symbol) {
            return Some((path.clone(), entry.clone()));
        }
    }

    None
}

pub fn map(mut callback: impl FnMut(&Path, &String, &IndexEntry)) {
    let index = unwrap!(WORKSPACE_INDEX.lock(), Err(error) => {
        error!("{:?}", error);
        return;
    });

    for (path, index) in index.iter() {
        for (symbol, entry) in index.iter() {
            let path = Path::new(path);
            callback(path, symbol, entry);
        }
    }
}

pub fn update(document: &Document, path: &Path) -> Result<bool> {
    index_document(document, path)
}

fn insert(path: &Path, entry: IndexEntry) {
    let mut index = unwrap!(WORKSPACE_INDEX.lock(), Err(error) => {
        error!("{:?}", error);
        return;
    });

    let path = unwrap!(path.to_str(), None => {
        error!("Couldn't convert path {} to string", path.display());
        return;
    });

    let index = index.entry(path.to_string()).or_default();
    index.insert(entry.key.clone(), entry);
}

// TODO: Should we consult the project .gitignore for ignored files?
// TODO: What about front-end ignores?
// TODO: What about other kinds of ignores (e.g. revdepcheck)?
pub fn filter_entry(entry: &DirEntry) -> bool {
    let name = entry.file_name();

    // skip common ignores
    for ignore in [".git", ".Rproj.user", "node_modules"] {
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

fn index_file(path: &Path) -> Result<bool> {
    // only index R files
    let ext = path.extension().unwrap_or_default();
    if ext != "r" && ext != "R" {
        return Ok(false);
    }

    // TODO: Handle document encodings here.
    // TODO: Check if there's an up-to-date buffer to be used.
    let contents = std::fs::read(path)?;
    let contents = String::from_utf8(contents)?;
    let document = Document::new(contents.as_str(), None);

    index_document(&document, path)?;

    Ok(true)
}

fn index_document(document: &Document, path: &Path) -> Result<bool> {
    let ast = &document.ast;
    let contents = &document.contents;

    let root = ast.root_node();
    let mut cursor = root.walk();
    for node in root.children(&mut cursor) {
        match index_node(path, contents, &node) {
            Ok(Some(entry)) => insert(path, entry),
            Ok(None) => {},
            Err(error) => error!("{:?}", error),
        }
    }

    Ok(true)
}

fn index_node(path: &Path, contents: &Rope, node: &Node) -> Result<Option<IndexEntry>> {
    if let Ok(Some(entry)) = index_function(path, contents, node) {
        return Ok(Some(entry));
    }

    if let Ok(Some(entry)) = index_comment(path, contents, node) {
        return Ok(Some(entry));
    }

    Ok(None)
}

fn index_function(_path: &Path, contents: &Rope, node: &Node) -> Result<Option<IndexEntry>> {
    // Check for assignment.
    matches!(node.kind(), "<-" | "=").into_result()?;

    // Check for identifier on left-hand side.
    let lhs = node.child_by_field_name("lhs").into_result()?;
    matches!(lhs.kind(), "identifier" | "string").into_result()?;

    // Check for a function definition on the right-hand side.
    let rhs = node.child_by_field_name("rhs").into_result()?;
    matches!(rhs.kind(), "function").into_result()?;

    let name = contents.node_slice(&lhs)?.to_string();
    let mut arguments = Vec::new();

    // Get the parameters node.
    let parameters = rhs.child_by_field_name("parameters").into_result()?;

    // Iterate through each, and get the names.
    let mut cursor = parameters.walk();
    for child in parameters.children(&mut cursor) {
        let name = unwrap!(child.child_by_field_name("name"), None => continue);
        if matches!(name.kind(), "identifier") {
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

fn index_comment(_path: &Path, contents: &Rope, node: &Node) -> Result<Option<IndexEntry>> {
    // check for comment
    matches!(node.kind(), "comment").into_result()?;

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
