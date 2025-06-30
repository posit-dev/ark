//
// symbols.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

#![allow(deprecated)]

use std::result::Result::Ok;

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
use crate::treesitter::point_end_of_previous_row;
use crate::treesitter::BinaryOperatorType;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

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

/// Represents a section in the document with its title, level, range, and children
#[derive(Debug)]
struct Section {
    title: String,
    level: usize,
    start_position: tree_sitter::Point,
    end_position: Option<tree_sitter::Point>,
    children: Vec<DocumentSymbol>,
}

pub(crate) fn document_symbols(
    state: &WorldState,
    params: &DocumentSymbolParams,
) -> anyhow::Result<Vec<DocumentSymbol>> {
    let uri = &params.text_document.uri;
    let document = state.documents.get(uri).into_result()?;
    let ast = &document.ast;
    let contents = &document.contents;

    // Start walking from the root node
    let root_node = ast.root_node();
    let mut result = Vec::new();

    // Extract and process all symbols from the AST
    if let Err(err) = collect_symbols(&root_node, contents, 0, &mut result) {
        log::error!("Failed to collect symbols: {err:?}");
        return Ok(Vec::new());
    }

    Ok(result)
}

/// Collect all document symbols from a node recursively
fn collect_symbols(
    node: &Node,
    contents: &Rope,
    current_level: usize,
    symbols: &mut Vec<DocumentSymbol>,
) -> anyhow::Result<()> {
    match node.node_type() {
        NodeType::Program | NodeType::BracedExpression => {
            collect_sections(node, contents, current_level, symbols)?;
        },

        NodeType::BinaryOperator(BinaryOperatorType::LeftAssignment) |
        NodeType::BinaryOperator(BinaryOperatorType::EqualsAssignment) => {
            collect_assignment(node, contents, symbols)?;
        },

        // For all other node types, no symbols need to be added
        _ => {},
    }

    Ok(())
}

fn collect_sections(
    node: &Node,
    contents: &Rope,
    current_level: usize,
    symbols: &mut Vec<DocumentSymbol>,
) -> anyhow::Result<()> {
    // In lists of expressions we track and collect section comments, then
    // collect symbols from children nodes

    let mut cursor = node.walk();

    // Track active sections at each level
    let mut active_sections: Vec<Section> = Vec::new();

    for child in node.children(&mut cursor) {
        if let NodeType::Comment = child.node_type() {
            let comment_text = contents.node_slice(&child)?.to_string();

            // If we have a section comment, add it to our stack and close any sections if needed
            if let Some((level, title)) = parse_comment_as_section(&comment_text) {
                let absolute_level = current_level + level;

                // Close any sections with equal or higher level
                while !active_sections.is_empty() &&
                    active_sections.last().unwrap().level >= absolute_level
                {
                    // Set end position for the section being closed
                    if let Some(section) = active_sections.last_mut() {
                        let pos = point_end_of_previous_row(child.start_position(), contents);
                        section.end_position = Some(pos);
                    }
                    finalize_section(&mut active_sections, symbols, contents)?;
                }

                let section = Section {
                    title,
                    level: absolute_level,
                    start_position: child.start_position(),
                    end_position: None,
                    children: Vec::new(),
                };
                active_sections.push(section);
            }

            continue;
        }

        // If we get to this point, `child` is not a section comment.
        // Recurse into child.

        if active_sections.is_empty() {
            // If no active section, extend current vector of symbols
            collect_symbols(&child, contents, current_level, symbols)?;
        } else {
            // Otherwise create new store of symbols for the current section
            let mut child_symbols = Vec::new();
            collect_symbols(&child, contents, current_level, &mut child_symbols)?;

            // Nest them inside last section
            if !child_symbols.is_empty() {
                active_sections
                    .last_mut()
                    .unwrap()
                    .children
                    .extend(child_symbols);
            }
        }
    }

    // Close any remaining active sections
    while !active_sections.is_empty() {
        // Set end position to the parent node's end for remaining sections
        if let Some(section) = active_sections.last_mut() {
            let mut pos = node.end_position();
            if pos.row > section.start_position.row {
                pos = point_end_of_previous_row(pos, contents);
            }
            section.end_position = Some(pos);
        }
        finalize_section(&mut active_sections, symbols, contents)?;
    }

    Ok(())
}

fn collect_assignment(
    node: &Node,
    contents: &Rope,
    symbols: &mut Vec<DocumentSymbol>,
) -> anyhow::Result<()> {
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
        return collect_assignment_with_function(node, contents, symbols);
    }

    // otherwise, just index as generic object
    let name = contents.node_slice(&lhs)?.to_string();

    let start = convert_point_to_position(contents, lhs.start_position());
    let end = convert_point_to_position(contents, lhs.end_position());

    let symbol = new_symbol(name, SymbolKind::VARIABLE, Range { start, end });
    symbols.push(symbol);

    Ok(())
}

fn collect_assignment_with_function(
    node: &Node,
    contents: &Rope,
    symbols: &mut Vec<DocumentSymbol>,
) -> anyhow::Result<()> {
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

    // Process the function body to extract child symbols
    let mut children = Vec::new();
    collect_symbols(&body, contents, 0, &mut children)?;

    let mut symbol = new_symbol_node(name, SymbolKind::FUNCTION, range, children);
    symbol.detail = Some(detail);
    symbols.push(symbol);

    Ok(())
}

/// Finalize a section by creating a symbol and adding it to the parent section or output
fn finalize_section(
    active_sections: &mut Vec<Section>,
    symbols: &mut Vec<DocumentSymbol>,
    contents: &Rope,
) -> anyhow::Result<()> {
    if let Some(section) = active_sections.pop() {
        let start_pos = section.start_position;
        let end_pos = section.end_position.unwrap_or(section.start_position);

        let range = Range {
            start: convert_point_to_position(contents, start_pos),
            end: convert_point_to_position(contents, end_pos),
        };

        let symbol = new_symbol(section.title, SymbolKind::STRING, range);

        let mut final_symbol = symbol;
        final_symbol.children = Some(section.children);

        if let Some(parent) = active_sections.last_mut() {
            parent.children.push(final_symbol);
        } else {
            symbols.push(final_symbol);
        }
    }

    Ok(())
}

// Function to parse a comment and return the section level and title
pub(crate) fn parse_comment_as_section(comment: &str) -> Option<(usize, String)> {
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

        let mut symbols = Vec::new();
        collect_symbols(&node, &doc.contents, 0, &mut symbols).unwrap();
        symbols
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

    #[test]
    fn test_symbol_section_ranges_extend() {
        let symbols = test_symbol(
            "# Section 1 ----
x <- 1
y <- 2
# Section 2 ----
z <- 3",
        );

        assert_eq!(symbols.len(), 2);

        // First section should extend from line 0 to line 2 (start of next section)
        let section1 = &symbols[0];
        assert_eq!(section1.name, "Section 1");
        assert_eq!(section1.range.start.line, 0);
        assert_eq!(section1.range.end.line, 2);

        // Second section should extend from line 3 to end of file
        let section2 = &symbols[1];
        assert_eq!(section2.name, "Section 2");
        assert_eq!(section2.range.start.line, 3);
        assert_eq!(section2.range.end.line, 3);
    }

    #[test]
    fn test_symbol_section_ranges_in_function() {
        let symbols = test_symbol(
            "foo <- function() {
  # Section A ----
  x <- 1
  y <- 2
  # Section B ----
  z <- 3
}",
        );

        assert_eq!(symbols.len(), 1);

        // Should have one function symbol
        let function = &symbols[0];
        assert_eq!(function.name, "foo");
        assert_eq!(function.kind, SymbolKind::FUNCTION);

        // Function should have two section children
        let children = function.children.as_ref().unwrap();
        assert_eq!(children.len(), 2);

        // First section should extend from line 1 to line 3 (start of next section)
        let section_a = &children[0];
        assert_eq!(section_a.name, "Section A");
        assert_eq!(section_a.range.start.line, 1);
        assert_eq!(section_a.range.end.line, 3);

        // Second section should extend from line 4 to end of function body
        let section_b = &children[1];
        assert_eq!(section_b.name, "Section B");
        assert_eq!(section_b.range.start.line, 4);
        assert_eq!(section_b.range.end.line, 5); // End of function body
    }
}
