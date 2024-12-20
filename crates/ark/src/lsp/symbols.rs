//
// symbols.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

#![allow(deprecated)]

use std::result::Result::Ok;

use anyhow::anyhow;
use ropey::Rope;
use stdext::unwrap::IntoResult;
use tower_lsp::lsp_types::DocumentSymbol;
use tower_lsp::lsp_types::DocumentSymbolParams;
use tower_lsp::lsp_types::Location;
use tower_lsp::lsp_types::Range;
use tower_lsp::lsp_types::SymbolInformation;
use tower_lsp::lsp_types::SymbolKind;
use tower_lsp::lsp_types::Url;
use tower_lsp::lsp_types::WorkspaceSymbolParams;
use tree_sitter::Node;

use crate::lsp::encoding::convert_point_to_position;
use crate::lsp::indexer;
use crate::lsp::indexer::IndexEntryData;
use crate::lsp::state::WorldState;
use crate::lsp::traits::rope::RopeExt;
use crate::lsp::traits::string::StringExt;
use crate::treesitter::BinaryOperatorType;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

type StoreStack = Vec<(usize, Option<DocumentSymbol>, Vec<DocumentSymbol>)>;

fn new_symbol(name: String, kind: SymbolKind, range: Range) -> DocumentSymbol {
    DocumentSymbol {
        name,
        kind,
        detail: None,
        children: Some(Vec::new()),
        deprecated: None,
        tags: None,
        range,
        selection_range: range,
    }
}

fn new_symbol_node(
    name: String,
    kind: SymbolKind,
    range: Range,
    children: Vec<DocumentSymbol>,
) -> DocumentSymbol {
    let mut symbol = new_symbol(name, kind, range);
    symbol.children = Some(children);
    symbol
}

pub fn symbols(params: &WorkspaceSymbolParams) -> anyhow::Result<Vec<SymbolInformation>> {
    let query = &params.query;
    let mut info: Vec<SymbolInformation> = Vec::new();

    indexer::map(|path, symbol, entry| {
        if !symbol.fuzzy_matches(query) {
            return;
        }

        match &entry.data {
            IndexEntryData::Function { name, arguments: _ } => {
                info.push(SymbolInformation {
                    name: name.to_string(),
                    kind: SymbolKind::FUNCTION,
                    location: Location {
                        uri: Url::from_file_path(path).unwrap(),
                        range: entry.range,
                    },
                    tags: None,
                    deprecated: None,
                    container_name: None,
                });
            },

            IndexEntryData::Section { level: _, title } => {
                info.push(SymbolInformation {
                    name: title.to_string(),
                    kind: SymbolKind::STRING,
                    location: Location {
                        uri: Url::from_file_path(path).unwrap(),
                        range: entry.range,
                    },
                    tags: None,
                    deprecated: None,
                    container_name: None,
                });
            },
            IndexEntryData::Variable { name } => {
                info.push(SymbolInformation {
                    name: name.clone(),
                    kind: SymbolKind::VARIABLE,
                    location: Location {
                        uri: Url::from_file_path(path).unwrap(),
                        range: entry.range,
                    },
                    tags: None,
                    deprecated: None,
                    container_name: None,
                });
            },
        };
    });

    Ok(info)
}

pub(crate) fn document_symbols(
    state: &WorldState,
    params: &DocumentSymbolParams,
) -> anyhow::Result<Vec<DocumentSymbol>> {
    let uri = &params.text_document.uri;
    let document = state.documents.get(uri).into_result()?;
    let ast = &document.ast;
    let contents = &document.contents;

    let node = ast.root_node();

    // Index from the root
    match index_node(&node, vec![], &contents) {
        Ok(children) => Ok(children),
        Err(err) => {
            log::error!("Error indexing node: {err:?}");
            return Ok(Vec::new());
        },
    }
}

fn index_node(
    node: &Node,
    store: Vec<DocumentSymbol>,
    contents: &Rope,
) -> anyhow::Result<Vec<DocumentSymbol>> {
    Ok(match node.node_type() {
        // Handle comment sections in expression lists
        NodeType::Program | NodeType::BracedExpression => {
            index_expression_list(&node, store, contents)?
        },
        // Index assignments as object or function symbols
        NodeType::BinaryOperator(BinaryOperatorType::LeftAssignment) |
        NodeType::BinaryOperator(BinaryOperatorType::EqualsAssignment) => {
            index_assignment(&node, store, contents)?
        },
        // Nothing to index. FIXME: We should handle argument lists, e.g. to
        // index inside functions passed as arguments, or inside `test_that()`
        // blocks.
        _ => store,
    })
}

// Handles root node and braced lists
fn index_expression_list(
    node: &Node,
    store: Vec<DocumentSymbol>,
    contents: &Rope,
) -> anyhow::Result<Vec<DocumentSymbol>> {
    let mut cursor = node.walk();

    // This is a stack of section levels and associated stores for comments of
    // the type `# title ----`. It contains all currently active sections.
    // The top-level section is the current store and has level 0. It should
    // always be in the stack and popping it before we have finished indexing
    // the whole expression list is a logic error.
    let mut store_stack: StoreStack = vec![(0, None, store)];

    for child in node.children(&mut cursor) {
        if let NodeType::Comment = child.node_type() {
            store_stack = index_comments(&child, store_stack, contents)?;
            continue;
        }

        // Get the current store to index the child subtree with.
        // We restore the store in the stack right after that.
        let Some((level, symbol, store)) = store_stack.pop() else {
            return Err(anyhow!(
                "Internal error: Store stack must have at least one element"
            ));
        };
        let store = index_node(&child, store, contents)?;
        store_stack.push((level, symbol, store));
    }

    // Pop all sections from the stack, assigning their childrens and their
    // parents along the way
    while store_stack.len() > 0 {
        if let Some(store) = store_stack_pop(&mut store_stack)? {
            return Ok(store);
        }
    }

    Err(anyhow!(
        "Internal error: Store stack must have at least one element"
    ))
}

// Pop store from the stack, recording its children and adding it as child to
// its parent (which becomes the last element in the stack).
fn store_stack_pop(store_stack: &mut StoreStack) -> anyhow::Result<Option<Vec<DocumentSymbol>>> {
    let Some((_, symbol, last)) = store_stack.pop() else {
        return Ok(None);
    };

    if let Some(mut sym) = symbol {
        // Assign children to symbol
        sym.children = Some(last);

        let Some((_, _, ref mut parent_store)) = store_stack.last_mut() else {
            return Err(anyhow!(
                "Internal error: Store stack must have at least one element"
            ));
        };

        // Store symbol as child of the last symbol on the stack
        parent_store.push(sym);

        Ok(None)
    } else {
        Ok(Some(last))
    }
}

fn index_comments(
    node: &Node,
    mut store_stack: StoreStack,
    contents: &Rope,
) -> anyhow::Result<StoreStack> {
    let comment_text = contents.node_slice(&node)?.to_string();

    // Check if the comment starts with one or more '#' followed by any text and ends with 4+ punctuations
    let Some((level, title)) = parse_comment_as_section(&comment_text) else {
        return Ok(store_stack);
    };

    // Create a section symbol based on the parsed comment
    let start = convert_point_to_position(contents, node.start_position());
    let end = convert_point_to_position(contents, node.end_position());
    let symbol = new_symbol(title, SymbolKind::STRING, Range { start, end });

    // Now pop all sections still on the stack that have a higher or equal
    // level. Because we pop sections with equal levels, i.e. siblings, we
    // ensure that there is only one active section per level on the stack.
    // That simplifies things because we need to assign popped sections to their
    // parents and we can assume the relevant parent is always the next on the
    // stack.
    loop {
        let Some((last_level, _, _)) = store_stack.last() else {
            return Err(anyhow!("Unexpectedly reached the end of the store stack"));
        };

        if *last_level >= level {
            if store_stack_pop(&mut store_stack)?.is_some() {
                return Err(anyhow!("Unexpectedly reached the end of the store stack"));
            }
            continue;
        }

        break;
    }

    store_stack.push((level, Some(symbol), vec![]));
    Ok(store_stack)
}

fn index_assignment(
    node: &Node,
    mut store: Vec<DocumentSymbol>,
    contents: &Rope,
) -> anyhow::Result<Vec<DocumentSymbol>> {
    // Check for assignment
    matches!(
        node.node_type(),
        NodeType::BinaryOperator(BinaryOperatorType::LeftAssignment) |
            NodeType::BinaryOperator(BinaryOperatorType::EqualsAssignment)
    )
    .into_result()?;

    // check for lhs, rhs
    let lhs = node.child_by_field_name("lhs").into_result()?;
    let rhs = node.child_by_field_name("rhs").into_result()?;

    // check for identifier on lhs, function on rhs
    let function = lhs.is_identifier_or_string() && rhs.is_function_definition();

    if function {
        return index_assignment_with_function(node, store, contents);
    }

    // otherwise, just index as generic object
    let name = contents.node_slice(&lhs)?.to_string();

    let start = convert_point_to_position(contents, lhs.start_position());
    let end = convert_point_to_position(contents, lhs.end_position());

    let symbol = new_symbol(name, SymbolKind::VARIABLE, Range { start, end });
    store.push(symbol);

    Ok(store)
}

fn index_assignment_with_function(
    node: &Node,
    mut store: Vec<DocumentSymbol>,
    contents: &Rope,
) -> anyhow::Result<Vec<DocumentSymbol>> {
    // check for lhs, rhs
    let lhs = node.child_by_field_name("lhs").into_result()?;
    let rhs = node.child_by_field_name("rhs").into_result()?;

    // start extracting the argument names
    let mut arguments: Vec<String> = Vec::new();
    let parameters = rhs.child_by_field_name("parameters").into_result()?;

    let mut cursor = parameters.walk();
    for parameter in parameters.children_by_field_name("parameter", &mut cursor) {
        let name = parameter.child_by_field_name("name").into_result()?;
        let name = contents.node_slice(&name)?.to_string();
        arguments.push(name);
    }

    let name = contents.node_slice(&lhs)?.to_string();
    let detail = format!("function({})", arguments.join(", "));

    let range = Range {
        start: convert_point_to_position(contents, lhs.start_position()),
        end: convert_point_to_position(contents, rhs.end_position()),
    };

    let body = rhs.child_by_field_name("body").into_result()?;

    // At this point we increase the nesting level. Recurse into the function
    // node with a new store of children nodes.
    let children = index_node(&body, vec![], contents)?;

    let mut symbol = new_symbol_node(name, SymbolKind::FUNCTION, range, children);
    symbol.detail = Some(detail);
    store.push(symbol);

    Ok(store)
}

// Function to parse a comment and return the section level and title
pub fn parse_comment_as_section(comment: &str) -> Option<(usize, String)> {
    // Match lines starting with one or more '#' followed by some non-empty content and must end with 4 or more '-', '#', or `=`
    // Ensure that there's actual content between the start and the trailing symbols.
    if let Some(caps) = indexer::RE_COMMENT_SECTION.captures(comment) {
        let hashes = caps.get(1)?.as_str().len(); // Count the number of '#'
        let title = caps.get(2)?.as_str().trim().to_string(); // Extract the title text without trailing punctuations
        if title.is_empty() {
            return None; // Return None for lines with only hashtags
        }
        return Some((hashes, title)); // Return the level based on the number of '#' and the title
    }

    None
}

#[cfg(test)]
mod tests {
    use tower_lsp::lsp_types::Position;

    use super::*;
    use crate::lsp::documents::Document;

    fn test_symbol(code: &str) -> Vec<DocumentSymbol> {
        let doc = Document::new(code, None);
        let node = doc.ast.root_node();

        index_node(&node, vec![], &doc.contents).unwrap()
    }

    #[test]
    fn test_symbol_parse_comment_as_section() {
        assert_eq!(parse_comment_as_section("# foo"), None);
        assert_eq!(parse_comment_as_section("# foo ---"), None);
        assert_eq!(parse_comment_as_section("########"), None);
        assert_eq!(
            parse_comment_as_section("# foo ----"),
            Some((1, String::from("foo")))
        );
    }

    #[test]
    fn test_symbol_comment_sections() {
        assert_eq!(test_symbol("# foo"), vec![]);
        assert_eq!(test_symbol("# foo ---"), vec![]);
        assert_eq!(test_symbol("########"), vec![]);

        let range = Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 10,
            },
        };
        assert_eq!(test_symbol("# foo ----"), vec![new_symbol(
            String::from("foo"),
            SymbolKind::STRING,
            range
        )]);
    }

    #[test]
    fn test_symbol_assignment() {
        let range = Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 3,
            },
        };
        assert_eq!(test_symbol("foo <- 1"), vec![new_symbol(
            String::from("foo"),
            SymbolKind::VARIABLE,
            range,
        )]);
    }

    #[test]
    fn test_symbol_assignment_function() {
        let range = Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 20,
            },
        };

        let mut foo = new_symbol(String::from("foo"), SymbolKind::FUNCTION, range);
        foo.detail = Some(String::from("function()"));

        assert_eq!(test_symbol("foo <- function() {}"), vec![foo]);
    }

    #[test]
    fn test_symbol_assignment_function_nested() {
        let range = Range {
            start: Position {
                line: 0,
                character: 20,
            },
            end: Position {
                line: 0,
                character: 23,
            },
        };
        let bar = new_symbol(String::from("bar"), SymbolKind::VARIABLE, range);

        let range = Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 30,
            },
        };
        let mut foo = new_symbol(String::from("foo"), SymbolKind::FUNCTION, range);
        foo.children = Some(vec![bar]);
        foo.detail = Some(String::from("function()"));

        assert_eq!(test_symbol("foo <- function() { bar <- 1 }"), vec![foo]);
    }

    #[test]
    fn test_symbol_assignment_function_nested_section() {
        insta::assert_debug_snapshot!(test_symbol(
            "
## title0 ----
foo <- function() {
  # title1 ----
  ### title2 ----
  ## title3 ----
  # title4 ----
}
# title5 ----"
        ));
    }

    #[test]
    fn test_symbol_braced_list() {
        let range = Range {
            start: Position {
                line: 0,
                character: 2,
            },
            end: Position {
                line: 0,
                character: 5,
            },
        };
        let foo = new_symbol(String::from("foo"), SymbolKind::VARIABLE, range);

        assert_eq!(test_symbol("{ foo <- 1 }"), vec![foo]);
    }
}
