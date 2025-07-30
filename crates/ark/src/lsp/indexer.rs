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
use tree_sitter::Query;
use walkdir::DirEntry;
use walkdir::WalkDir;

use crate::lsp;
use crate::lsp::documents::Document;
use crate::lsp::encoding::convert_point_to_position;
use crate::lsp::traits::rope::RopeExt;
use crate::treesitter::BinaryOperatorType;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;
use crate::treesitter::TsQuery;

#[derive(Clone, Debug)]
pub enum IndexEntryData {
    Variable {
        name: String,
    },
    Function {
        name: String,
        arguments: Vec<String>,
    },
    // Like Function but not used for completions yet
    Method {
        name: String,
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
                    if let Err(err) = create(entry.path()) {
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

/// Search the workspace files and return the first symbol match
pub fn find(symbol: &str) -> Option<(String, IndexEntry)> {
    let index = WORKSPACE_INDEX.lock().unwrap();

    for (path, index) in index.iter() {
        if let Some(entry) = index.get(symbol) {
            return Some((path.clone(), entry.clone()));
        }
    }

    None
}

/// Search a specific workspace file for a symbol
pub fn find_in_file(symbol: &str, path: &std::path::Path) -> Option<(String, IndexEntry)> {
    let index = WORKSPACE_INDEX.lock().unwrap();

    if let Ok(path_str) = str_from_path(path) {
        if let Some(index) = index.get(path_str) {
            if let Some(entry) = index.get(symbol) {
                return Some((path_str.to_string(), entry.clone()));
            }
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
    delete(path)?;
    index_document(document, path);
    Ok(())
}

fn insert(path: &Path, entry: IndexEntry) -> anyhow::Result<()> {
    let mut index = WORKSPACE_INDEX.lock().unwrap();
    let path = str_from_path(path)?;

    let index = index.entry(path.to_string()).or_default();
    index_insert(index, entry);

    Ok(())
}

fn index_insert(index: &mut HashMap<String, IndexEntry>, entry: IndexEntry) {
    // We generally retain only the first occurrence in the index. In the
    // future we'll track every occurrences and their scopes but for now we
    // only track the first definition of an object (in a way, its
    // declaration).
    if let Some(existing_entry) = index.get(&entry.key) {
        // Give priority to non-section entries.
        if matches!(existing_entry.data, IndexEntryData::Section { .. }) {
            index.insert(entry.key.clone(), entry);
        }
        // Else, ignore.
    } else {
        index.insert(entry.key.clone(), entry);
    }
}

#[tracing::instrument(level = "trace")]
pub(crate) fn delete(path: &Path) -> anyhow::Result<()> {
    let mut index = WORKSPACE_INDEX.lock().unwrap();
    let path = str_from_path(path)?;

    // Only clears if the `path` was an existing key
    index.entry(path.into()).and_modify(|index| {
        index.clear();
    });

    Ok(())
}

#[tracing::instrument(level = "trace")]
pub(crate) fn rename(old: &Path, new: &Path) -> anyhow::Result<()> {
    let mut index = WORKSPACE_INDEX.lock().unwrap();
    let old = str_from_path(old)?;
    let new = str_from_path(new)?;

    if let Some(entries) = index.remove(old) {
        index.insert(new.to_string(), entries);
    }

    Ok(())
}

#[cfg(test)]
pub(crate) fn indexer_clear() {
    let mut index = WORKSPACE_INDEX.lock().unwrap();
    index.clear();
}

/// RAII guard that clears `WORKSPACE_INDEX` when dropped.
/// Useful for ensuring a clean index state in tests.
#[cfg(test)]
pub(crate) struct ResetIndexerGuard;

#[cfg(test)]
impl Drop for ResetIndexerGuard {
    fn drop(&mut self) {
        indexer_clear();
    }
}

fn str_from_path(path: &Path) -> anyhow::Result<&str> {
    path.to_str().ok_or(anyhow!(
        "Couldn't convert path {} to string",
        path.to_string_lossy()
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

pub(crate) fn create(path: &Path) -> anyhow::Result<()> {
    // Only index R files
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
    let mut entries = Vec::new();

    for node in root.children(&mut cursor) {
        if let Err(err) = index_node(path, contents, &node, &mut entries) {
            lsp::log_error!("Can't index document: {err:?}");
        }
    }

    for entry in entries {
        if let Err(err) = insert(path, entry) {
            lsp::log_error!("Can't insert index entry: {err:?}");
        }
    }
}

fn index_node(
    path: &Path,
    contents: &Rope,
    node: &Node,
    entries: &mut Vec<IndexEntry>,
) -> anyhow::Result<()> {
    index_assignment(path, contents, node, entries)?;
    index_comment(path, contents, node, entries)?;
    Ok(())
}

fn index_assignment(
    path: &Path,
    contents: &Rope,
    node: &Node,
    entries: &mut Vec<IndexEntry>,
) -> anyhow::Result<()> {
    if !matches!(
        node.node_type(),
        NodeType::BinaryOperator(BinaryOperatorType::LeftAssignment) |
            NodeType::BinaryOperator(BinaryOperatorType::EqualsAssignment)
    ) {
        return Ok(());
    }

    let lhs = match node.child_by_field_name("lhs") {
        Some(lhs) => lhs,
        None => return Ok(()),
    };

    let Some(rhs) = node.child_by_field_name("rhs") else {
        return Ok(());
    };

    if crate::treesitter::node_is_call(&rhs, "R6Class", contents) ||
        crate::treesitter::node_is_namespaced_call(&rhs, "R6", "R6Class", contents)
    {
        index_r6_class_methods(path, contents, &rhs, entries)?;
        // Fallthrough to index the variable to which the R6 class is assigned
    }

    let lhs_text = contents.node_slice(&lhs)?.to_string();

    // The method matching is super hacky but let's wait until the typed API to
    // do better
    if !lhs_text.starts_with("method(") && !lhs.is_identifier_or_string() {
        return Ok(());
    }

    let Some(rhs) = node.child_by_field_name("rhs") else {
        return Ok(());
    };

    if rhs.is_function_definition() {
        // If RHS is a function definition, emit a function symbol
        let mut arguments = Vec::new();
        if let Some(parameters) = rhs.child_by_field_name("parameters") {
            let mut cursor = parameters.walk();
            for child in parameters.children(&mut cursor) {
                let name = unwrap!(child.child_by_field_name("name"), None => continue);
                if name.is_identifier() {
                    let name = contents.node_slice(&name)?.to_string();
                    arguments.push(name);
                }
            }
        }

        // Note that unlike document symbols whose ranges cover the whole entity
        // they represent, the range of workspace symbols only cover the identifers
        let start = convert_point_to_position(contents, lhs.start_position());
        let end = convert_point_to_position(contents, lhs.end_position());

        entries.push(IndexEntry {
            key: lhs_text.clone(),
            range: Range { start, end },
            data: IndexEntryData::Function {
                name: lhs_text,
                arguments,
            },
        });
    } else {
        // Otherwise, emit variable
        let start = convert_point_to_position(contents, lhs.start_position());
        let end = convert_point_to_position(contents, lhs.end_position());
        entries.push(IndexEntry {
            key: lhs_text.clone(),
            range: Range { start, end },
            data: IndexEntryData::Variable { name: lhs_text },
        });
    }

    Ok(())
}

fn index_r6_class_methods(
    _path: &Path,
    contents: &Rope,
    node: &Node,
    entries: &mut Vec<IndexEntry>,
) -> anyhow::Result<()> {
    // Tree-sitter query to match individual methods in R6Class public/private lists
    static R6_METHODS_QUERY: LazyLock<Query> = LazyLock::new(|| {
        let query_str = r#"
            (argument
                name: (identifier) @access
                value: (call
                    function: (identifier) @_list_fn
                    arguments: (arguments
                        (argument
                            name: (identifier) @method_name
                            value: (function_definition) @method_fn
                        )
                    )
                )
                (#match? @access "public|private")
                (#eq? @_list_fn "list")
            )
        "#;
        let language = &tree_sitter_r::LANGUAGE.into();
        Query::new(language, query_str).expect("Failed to compile R6 methods query")
    });
    let mut ts_query = TsQuery::from_query(&*R6_METHODS_QUERY);

    // We'll switch from Rope to String in the near future so let's not
    // worry about this conversion now
    let contents_str = contents.to_string();

    for method_node in ts_query.captures_for(*node, "method_name", contents_str.as_bytes()) {
        let name = contents.node_slice(&method_node)?.to_string();
        let start = convert_point_to_position(contents, method_node.start_position());
        let end = convert_point_to_position(contents, method_node.end_position());

        entries.push(IndexEntry {
            key: name.clone(),
            range: Range { start, end },
            data: IndexEntryData::Method { name },
        });
    }

    Ok(())
}

fn index_comment(
    _path: &Path,
    contents: &Rope,
    node: &Node,
    entries: &mut Vec<IndexEntry>,
) -> anyhow::Result<()> {
    // check for comment
    if !node.is_comment() {
        return Ok(());
    }

    // see if it looks like a section
    let comment = contents.node_slice(node)?.to_string();
    let matches = match RE_COMMENT_SECTION.captures(comment.as_str()) {
        Some(m) => m,
        None => return Ok(()),
    };

    let level = matches.get(1).into_result()?;
    let title = matches.get(2).into_result()?;

    let level = level.as_str().len();
    let title = title.as_str().to_string();

    // skip things that look like knitr output
    if title.starts_with("----") {
        return Ok(());
    }

    let start = convert_point_to_position(contents, node.start_position());
    let end = convert_point_to_position(contents, node.end_position());

    entries.push(IndexEntry {
        key: title.clone(),
        range: Range::new(start, end),
        data: IndexEntryData::Section { level, title },
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use assert_matches::assert_matches;
    use insta::assert_debug_snapshot;
    use tower_lsp::lsp_types;

    use super::*;
    use crate::lsp::documents::Document;

    macro_rules! test_index {
        ($code:expr) => {
            let doc = Document::new($code, None);
            let path = PathBuf::from("/path/to/file.R");
            let root = doc.ast.root_node();
            let mut cursor = root.walk();

            let mut entries = vec![];
            for node in root.children(&mut cursor) {
                let _ = index_node(&path, &doc.contents, &node, &mut entries);
            }
            assert_debug_snapshot!(entries);
        };
    }

    // Note that unlike document symbols whose ranges cover the whole entity
    // they represent, the range of workspace symbols only cover the identifers

    #[test]
    fn test_index_function() {
        test_index!(
            r#"
my_function <- function(a, b = 1) {
  a + b

  # These are not indexed as workspace symbol
  inner <- function() {
    2
  }
  inner_var <- 3
}

my_variable <- 1
"#
        );
    }

    #[test]
    fn test_index_variable() {
        test_index!(
            r#"
x <- 10
y = "hello"
"#
        );
    }

    #[test]
    fn test_index_s7_methods() {
        test_index!(
            r#"
Class <- new_class("Class")
generic <- new_generic("generic", "arg",
  function(arg) {
    S7_dispatch()
  }
)
method(generic, Class) <- function(arg) {
  NULL
}
"#
        );
    }

    #[test]
    fn test_index_comment_section() {
        test_index!(
            r#"
# Section 1 ----
x <- 10

## Subsection ======
y <- 20

x <- function() {
    # This inner section is not indexed ----
}

"#
        );
    }

    #[test]
    fn test_index_r6class() {
        test_index!(
            r#"
class <- R6Class(
    public = list(
        initialize = function() {
            1
        },
        public_method = function() {
            2
        },
        public_variable = NA
    ),
    private = list(
        private_method = function() {
            1
        },
        private_variable = NA
    ),
    other = list(
        other_method = function() {
            1
        }
    )
)
"#
        );
    }

    #[test]
    fn test_index_r6class_namespaced() {
        test_index!(
            r#"
class <- R6::R6Class(
    public = list(
        initialize = function() {
            1
        },
    )
)
"#
        );
    }

    #[test]
    fn test_index_insert_priority() {
        let mut index = HashMap::new();

        let section_entry = IndexEntry {
            key: "foo".to_string(),
            range: Range::new(
                lsp_types::Position::new(0, 0),
                lsp_types::Position::new(0, 3),
            ),
            data: IndexEntryData::Section {
                level: 1,
                title: "foo".to_string(),
            },
        };

        let variable_entry = IndexEntry {
            key: "foo".to_string(),
            range: Range::new(
                lsp_types::Position::new(1, 0),
                lsp_types::Position::new(1, 3),
            ),
            data: IndexEntryData::Variable {
                name: "foo".to_string(),
            },
        };

        // The Variable has priority and should replace the Section
        index_insert(&mut index, section_entry.clone());
        index_insert(&mut index, variable_entry.clone());
        assert_matches!(
            &index.get("foo").unwrap().data,
            IndexEntryData::Variable { name } => assert_eq!(name, "foo")
        );

        // Inserting a Section again with the same key does not override the Variable
        index_insert(&mut index, section_entry.clone());
        assert_matches!(
            &index.get("foo").unwrap().data,
            IndexEntryData::Variable { name } => assert_eq!(name, "foo")
        );

        let function_entry = IndexEntry {
            key: "foo".to_string(),
            range: Range::new(
                lsp_types::Position::new(2, 0),
                lsp_types::Position::new(2, 3),
            ),
            data: IndexEntryData::Function {
                name: "foo".to_string(),
                arguments: vec!["a".to_string()],
            },
        };

        // Inserting another kind of variable (e.g., Function) with the same key
        // does not override it either. The first occurrence is generally retained.
        index_insert(&mut index, function_entry.clone());
        assert_matches!(
            &index.get("foo").unwrap().data,
            IndexEntryData::Variable { name } => assert_eq!(name, "foo")
        );
    }
}
