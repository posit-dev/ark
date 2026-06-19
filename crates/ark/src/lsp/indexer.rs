//
// indexer.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::result::Result::Ok;
use std::sync::LazyLock;

use aether_lsp_utils::proto::to_proto;
use aether_lsp_utils::proto::PositionEncoding;
use oak_db::File;
use regex::Regex;
use stdext::result::ResultExt;
use stdext::unwrap;
use stdext::unwrap::IntoResult;
use tower_lsp::lsp_types::Range;
use tree_sitter::Node;
use tree_sitter::Query;

use crate::lsp;
use crate::lsp::db::ArkDb;
use crate::lsp::db::FileArkExt;
use crate::lsp::traits::node::NodeExt;
use crate::treesitter::BinaryOperatorType;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;
use crate::treesitter::TsQuery;

#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
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

/// A position in a file as a tree-sitter point: zero-based row, and column in
/// bytes within that row. Encoding-free, so the per-file index stays a pure
/// salsa query. Consumers that need an LSP position convert via the file's
/// line index and the session encoding (see `index_range_to_lsp_range`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, salsa::Update)]
pub struct IndexPoint {
    pub row: u32,
    pub column: u32,
}

impl From<tree_sitter::Point> for IndexPoint {
    fn from(point: tree_sitter::Point) -> Self {
        Self {
            row: point.row as u32,
            column: point.column as u32,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, salsa::Update)]
pub struct IndexRange {
    pub start: IndexPoint,
    pub end: IndexPoint,
}

#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct IndexEntry {
    pub key: String,
    pub range: IndexRange,
    pub data: IndexEntryData,
}

/// Convert an index entry's tree-sitter point range to an LSP range, resolving
/// the file's line index from the db. Returns `None` if the points fall outside
/// the line index, in which case the symbol is dropped from the results.
pub(crate) fn index_range_to_lsp_range(
    db: &dyn ArkDb,
    file: File,
    range: IndexRange,
    encoding: PositionEncoding,
) -> Option<Range> {
    let line_index = file.line_index(db);

    let to_position = |point: IndexPoint| {
        to_proto::position_from_line_col(
            biome_line_index::LineCol {
                line: point.row,
                col: point.column,
            },
            line_index,
            encoding,
        )
        .log_err()
    };

    Some(Range::new(
        to_position(range.start)?,
        to_position(range.end)?,
    ))
}

pub static RE_COMMENT_SECTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(#+)\s*(.*?)\s*[#=-]{4,}\s*$").unwrap());

/// One file's workspace symbols, keyed by symbol name. Built by [`file_index`].
#[derive(Clone, Debug, Default, PartialEq, Eq, salsa::Update)]
pub(crate) struct FileIndex {
    pub(crate) symbols: rustc_hash::FxHashMap<String, IndexEntry>,
}

/// Find the first workspace symbol matching `symbol`, scanning files in
/// `workspace_files` order. `ark://` virtual docs are skipped: they show
/// foreign code the user can't edit.
pub(crate) fn find(db: &dyn ArkDb, symbol: &str) -> Option<IndexEntry> {
    for &file in oak_db::workspace_files(db) {
        if !is_indexable(db, file) {
            continue;
        }
        if let Some(entry) = file_index(db, file).symbols.get(symbol) {
            return Some(entry.clone());
        }
    }
    None
}

/// Extract a file's workspace symbols.
#[salsa::tracked(returns(ref))]
fn file_index(db: &dyn ArkDb, file: File) -> FileIndex {
    let tree = file.tree_sitter(db);
    let contents = file.source_text(db).as_str();

    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut entries = Vec::new();

    for node in root.children(&mut cursor) {
        if let Err(err) = index_node(contents, &node, &mut entries) {
            lsp::log_error!("Can't index document: {err:?}");
        }
    }

    let mut symbols = rustc_hash::FxHashMap::default();
    for entry in entries {
        index_insert(&mut symbols, entry);
    }

    FileIndex { symbols }
}

/// Visit every workspace symbol across all indexable files. Callers that need a
/// URL for messages to the frontend can resolve it from `File` via
/// `WorldState::wire_url`.
pub(crate) fn map(db: &dyn ArkDb, mut callback: impl FnMut(File, &str, &IndexEntry)) {
    for &file in oak_db::workspace_files(db) {
        if !is_indexable(db, file) {
            continue;
        }
        for (symbol, entry) in file_index(db, file).symbols.iter() {
            callback(file, symbol, entry);
        }
    }
}

/// Call [`file_index()`] for every workspace file. This ensures workspace
/// symbols are loaded before the user needs to read them (e.g. by looking up a
/// workspace symbol without any file opened).
pub(crate) fn warm(db: &dyn ArkDb) {
    for &file in oak_db::workspace_files(db) {
        file_index(db, file);
    }
}

fn is_indexable(db: &dyn ArkDb, file: File) -> bool {
    match file.path(db) {
        aether_path::FilePath::File(_) => true,
        aether_path::FilePath::Virtual(uri) => uri.as_url().scheme() != "ark",
    }
}

fn index_insert(index: &mut rustc_hash::FxHashMap<String, IndexEntry>, entry: IndexEntry) {
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

fn index_node(contents: &str, node: &Node, entries: &mut Vec<IndexEntry>) -> anyhow::Result<()> {
    index_assignment(contents, node, entries)?;
    index_comment(contents, node, entries)?;
    Ok(())
}

fn index_assignment(
    contents: &str,
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
        index_r6_class_methods(contents, &rhs, entries)?;
        // Fallthrough to index the variable to which the R6 class is assigned
    }

    let lhs_text = lhs.node_to_string(contents)?;

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
                    let name = name.node_to_string(contents)?;
                    arguments.push(name);
                }
            }
        }

        // Note that unlike document symbols whose ranges cover the whole entity
        // they represent, the range of workspace symbols only cover the identifers
        entries.push(IndexEntry {
            key: lhs_text.clone(),
            range: IndexRange {
                start: lhs.start_position().into(),
                end: lhs.end_position().into(),
            },
            data: IndexEntryData::Function {
                name: lhs_text,
                arguments,
            },
        });
    } else {
        // Otherwise, emit variable
        entries.push(IndexEntry {
            key: lhs_text.clone(),
            range: IndexRange {
                start: lhs.start_position().into(),
                end: lhs.end_position().into(),
            },
            data: IndexEntryData::Variable { name: lhs_text },
        });
    }

    Ok(())
}

fn index_r6_class_methods(
    contents: &str,
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
    let mut ts_query = TsQuery::from_query(&R6_METHODS_QUERY);

    for method_node in ts_query.captures_for(*node, "method_name", contents.as_bytes()) {
        let name = method_node.node_to_string(contents)?;

        entries.push(IndexEntry {
            key: name.clone(),
            range: IndexRange {
                start: method_node.start_position().into(),
                end: method_node.end_position().into(),
            },
            data: IndexEntryData::Method { name },
        });
    }

    Ok(())
}

fn index_comment(contents: &str, node: &Node, entries: &mut Vec<IndexEntry>) -> anyhow::Result<()> {
    // check for comment
    if !node.is_comment() {
        return Ok(());
    }

    // see if it looks like a section
    let comment = node.node_as_str(contents)?;
    let matches = match RE_COMMENT_SECTION.captures(comment) {
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

    entries.push(IndexEntry {
        key: title.clone(),
        range: IndexRange {
            start: node.start_position().into(),
            end: node.end_position().into(),
        },
        data: IndexEntryData::Section { level, title },
    });

    Ok(())
}

#[cfg(test)]
mod tests {

    use assert_matches::assert_matches;
    use insta::assert_debug_snapshot;
    use oak_scan::DbScan;
    use url::Url;

    use super::*;
    use crate::lsp::ark_file::test_ark_file;

    macro_rules! test_index {
        ($code:expr) => {
            let (db, file) = test_ark_file($code);
            let tree = file.tree_sitter(&db);
            let contents = file.contents(&db);
            let root = tree.root_node();
            let mut cursor = root.walk();

            let mut entries = vec![];
            for node in root.children(&mut cursor) {
                let _ = index_node(contents, &node, &mut entries);
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
        let mut index = rustc_hash::FxHashMap::default();

        let section_entry = IndexEntry {
            key: "foo".to_string(),
            range: IndexRange {
                start: IndexPoint { row: 0, column: 0 },
                end: IndexPoint { row: 0, column: 3 },
            },
            data: IndexEntryData::Section {
                level: 1,
                title: "foo".to_string(),
            },
        };

        let variable_entry = IndexEntry {
            key: "foo".to_string(),
            range: IndexRange {
                start: IndexPoint { row: 1, column: 0 },
                end: IndexPoint { row: 1, column: 3 },
            },
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
            range: IndexRange {
                start: IndexPoint { row: 2, column: 0 },
                end: IndexPoint { row: 2, column: 3 },
            },
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

    #[test]
    fn test_index_skips_ark_virtual_doc() {
        use aether_path::FilePath;
        let mut db = oak_db::OakDatabase::new();
        let url = Url::parse("ark://namespace/test.R").unwrap();
        db.upsert_editor(FilePath::from_url(&url), "foo <- 1".to_string());
        assert!(find(&db, "foo").is_none());
    }

    #[test]
    fn test_index_indexes_git_uri() {
        use aether_path::FilePath;
        let mut db = oak_db::OakDatabase::new();
        let url = Url::parse("git:///home/user/test.R?ref=HEAD").unwrap();
        db.upsert_editor(FilePath::from_url(&url), "foo <- 1".to_string());
        assert!(find(&db, "foo").is_some());
    }
}
