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

pub(crate) fn symbols(
    params: &WorkspaceSymbolParams,
    state: &WorldState,
) -> anyhow::Result<Vec<SymbolInformation>> {
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
                if state.config.workspace_symbols.include_comment_sections {
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
                }
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

            IndexEntryData::Method { name } => {
                info.push(SymbolInformation {
                    name: name.clone(),
                    kind: SymbolKind::METHOD,
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

struct CollectContext {
    top_level: bool,
    include_assignments_in_blocks: bool,
}

impl CollectContext {
    fn new() -> Self {
        Self {
            top_level: true,
            include_assignments_in_blocks: false,
        }
    }
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

    let mut ctx = CollectContext::new();
    ctx.include_assignments_in_blocks = state.config.symbols.include_assignments_in_blocks;

    // Extract and process all symbols from the AST
    if let Err(err) = collect_symbols(&mut ctx, &root_node, contents, 0, &mut result) {
        log::error!("Failed to collect symbols: {err:?}");
        return Ok(Vec::new());
    }

    Ok(result)
}

/// Collect all document symbols from a node recursively
fn collect_symbols(
    ctx: &mut CollectContext,
    node: &Node,
    contents: &Rope,
    current_level: usize,
    symbols: &mut Vec<DocumentSymbol>,
) -> anyhow::Result<()> {
    match node.node_type() {
        NodeType::Program => {
            collect_sections(ctx, node, contents, current_level, symbols)?;
        },

        NodeType::BracedExpression => {
            ctx.top_level = false;
            collect_sections(ctx, node, contents, current_level, symbols)?;
        },

        NodeType::Call => {
            collect_call(ctx, node, contents, symbols)?;
        },

        NodeType::BinaryOperator(BinaryOperatorType::LeftAssignment) |
        NodeType::BinaryOperator(BinaryOperatorType::EqualsAssignment) => {
            collect_assignment(ctx, node, contents, symbols)?;
        },

        // For all other node types, no symbols need to be added
        _ => {},
    }
    Ok(())
}

fn collect_sections(
    ctx: &mut CollectContext,
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
            collect_symbols(ctx, &child, contents, current_level, symbols)?;
        } else {
            // Otherwise create new store of symbols for the current section
            let mut child_symbols = Vec::new();
            collect_symbols(ctx, &child, contents, current_level, &mut child_symbols)?;

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

fn collect_call(
    ctx: &mut CollectContext,
    node: &Node,
    contents: &Rope,
    symbols: &mut Vec<DocumentSymbol>,
) -> anyhow::Result<()> {
    let Some(callee) = node.child_by_field_name("function") else {
        return Ok(());
    };

    if callee.is_identifier() {
        let fun_symbol = contents.node_slice(&callee)?.to_string();
        match fun_symbol.as_str() {
            "test_that" => return collect_call_test_that(ctx, node, contents, symbols),
            _ => {}, // fallthrough
        }
    }

    collect_call_arguments(ctx, node, contents, symbols)?;

    Ok(())
}

fn collect_call_arguments(
    ctx: &mut CollectContext,
    node: &Node,
    contents: &Rope,
    symbols: &mut Vec<DocumentSymbol>,
) -> anyhow::Result<()> {
    let Some(arguments) = node.child_by_field_name("arguments") else {
        return Ok(());
    };

    let mut cursor = node.walk();
    for arg in arguments.children_by_field_name("argument", &mut cursor) {
        let Some(arg_value) = arg.child_by_field_name("value") else {
            continue;
        };

        match arg_value.kind() {
            "function_definition" => {
                if let Some(arg_fun) = arg.child_by_field_name("name") {
                    // If this is a named function, collect it as a method
                    collect_method(ctx, &arg_fun, &arg_value, contents, symbols)?;
                } else {
                    // Otherwise, just recurse into the function
                    let body = arg_value.child_by_field_name("body").into_result()?;
                    collect_symbols(ctx, &body, contents, 0, symbols)?;
                };
            },
            _ => {
                // Recurse into arguments. They might be a braced list, another call
                // that might contain functions, etc.
                collect_symbols(ctx, &arg_value, contents, 0, symbols)?;
            },
        }
    }

    Ok(())
}

fn collect_method(
    ctx: &mut CollectContext,
    arg_fun: &Node,
    arg_value: &Node,
    contents: &Rope,
    symbols: &mut Vec<DocumentSymbol>,
) -> anyhow::Result<()> {
    if !arg_fun.is_identifier_or_string() {
        return Ok(());
    }
    let arg_name_str = contents.node_slice(&arg_fun)?.to_string();

    let start = convert_point_to_position(contents, arg_value.start_position());
    let end = convert_point_to_position(contents, arg_value.end_position());

    let body = arg_value.child_by_field_name("body").into_result()?;
    let mut children = vec![];
    collect_symbols(ctx, &body, contents, 0, &mut children)?;

    let mut symbol = new_symbol_node(
        arg_name_str,
        SymbolKind::METHOD,
        Range { start, end },
        children,
    );

    // Don't include whole function as detail as the body often doesn't
    // provide useful information and only make the outline more busy (with
    // curly braces, newline characters, etc).
    symbol.detail = Some(String::from("function()"));

    symbols.push(symbol);

    Ok(())
}

// https://github.com/posit-dev/positron/issues/1428
fn collect_call_test_that(
    ctx: &mut CollectContext,
    node: &Node,
    contents: &Rope,
    symbols: &mut Vec<DocumentSymbol>,
) -> anyhow::Result<()> {
    let Some(arguments) = node.child_by_field_name("arguments") else {
        return Ok(());
    };

    // We don't do any argument matching and just consider the first argument if
    // a string. First skip over `(`.
    let Some(first_argument) = arguments.child(1).and_then(|n| n.child(0)) else {
        return Ok(());
    };
    if !first_argument.is_string() {
        return Ok(());
    }

    let Some(string) = first_argument.child_by_field_name("content") else {
        return Ok(());
    };

    // Recurse in arguments. We could skip the first one if we wanted.
    let mut children = Vec::new();
    let mut cursor = arguments.walk();
    for child in arguments.children_by_field_name("argument", &mut cursor) {
        if let Some(value) = child.child_by_field_name("value") {
            collect_symbols(ctx, &value, contents, 0, &mut children)?;
        }
    }

    let name = contents.node_slice(&string)?.to_string();
    let name = format!("Test: {name}");

    let start = convert_point_to_position(contents, node.start_position());
    let end = convert_point_to_position(contents, node.end_position());

    let symbol = new_symbol_node(name, SymbolKind::FUNCTION, Range { start, end }, children);
    symbols.push(symbol);

    Ok(())
}

fn collect_assignment(
    ctx: &mut CollectContext,
    node: &Node,
    contents: &Rope,
    symbols: &mut Vec<DocumentSymbol>,
) -> anyhow::Result<()> {
    let (NodeType::BinaryOperator(BinaryOperatorType::LeftAssignment) |
    NodeType::BinaryOperator(BinaryOperatorType::EqualsAssignment)) = node.node_type()
    else {
        return Ok(());
    };

    let (Some(lhs), Some(rhs)) = (
        node.child_by_field_name("lhs"),
        node.child_by_field_name("rhs"),
    ) else {
        return Ok(());
    };

    // If a function, collect symbol as function
    let function = lhs.is_identifier_or_string() && rhs.is_function_definition();
    if function {
        return collect_assignment_with_function(ctx, node, contents, symbols);
    }

    if ctx.top_level || ctx.include_assignments_in_blocks {
        // Collect as generic object, but typically only if we're at top-level. Assigned
        // objects in nested functions and blocks cause the outline to become
        // too busy.
        let name = contents.node_slice(&lhs)?.to_string();

        let start = convert_point_to_position(contents, node.start_position());
        let end = convert_point_to_position(contents, node.end_position());

        // Now recurse into RHS
        let mut children = Vec::new();
        collect_symbols(ctx, &rhs, contents, 0, &mut children)?;

        let symbol = new_symbol_node(name, SymbolKind::VARIABLE, Range { start, end }, children);
        symbols.push(symbol);
    } else {
        // Recurse into RHS
        collect_symbols(ctx, &rhs, contents, 0, symbols)?;
    }

    Ok(())
}

fn collect_assignment_with_function(
    ctx: &mut CollectContext,
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
    collect_symbols(ctx, &body, contents, 0, &mut children)?;

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
    use crate::lsp::config::LspConfig;
    use crate::lsp::config::WorkspaceSymbolsConfig;
    use crate::lsp::documents::Document;
    use crate::lsp::indexer::IndexerGuard;

    fn test_symbol(code: &str) -> Vec<DocumentSymbol> {
        let doc = Document::new(code, None);
        let node = doc.ast.root_node();

        let mut symbols = Vec::new();
        collect_symbols(
            &mut CollectContext::new(),
            &node,
            &doc.contents,
            0,
            &mut symbols,
        )
        .unwrap();
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
                character: 8,
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
        insta::assert_debug_snapshot!(test_symbol("foo <- function() { bar <- function() 1 }"));
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

    #[test]
    fn test_symbol_call_test_that() {
        insta::assert_debug_snapshot!(test_symbol(
            "
test_that_not('foo', {
  1
})

# title ----

test_that('foo', {
  # title1 ----
  1
  # title2 ----
  foo <- function() {
    2
  }
})

# title2 ----
test_that('bar', {
  1
})
"
        ));
    }

    #[test]
    fn test_symbol_call_methods() {
        insta::assert_debug_snapshot!(test_symbol(
            "
# section ----
list(
    foo = function() {
        1
        # nested section ----
        nested <- function() {}
    }, # matched
    function() {
        2
        # `nested` is a symbol even if the unnamed method is not
        nested <- function () {
    }
    }, # not matched
    bar = function() {
        3
    }, # matched
    baz = (function() {
        4
    }) # not matched
)
"
        ));
    }

    #[test]
    fn test_symbol_call_arguments() {
        insta::assert_debug_snapshot!(test_symbol(
            "
# section ----
local({
    a <- function() {
        1
    }
})
"
        ));
    }

    #[test]
    fn test_symbol_rhs_braced_list() {
        insta::assert_debug_snapshot!(test_symbol(
            "
foo <- {
    bar <- function() {}
}
"
        ));
    }

    #[test]
    fn test_symbol_rhs_methods() {
        insta::assert_debug_snapshot!(test_symbol(
            "
# section ----
class <- r6::r6class(
  'class',
  public = list(
    initialize = function() 'initialize',
    foo = function() 'foo'
  ),
  private = list(
    bar = function() 'bar'
  )
)
"
        ));
    }

    #[test]
    // Assigned variables in nested contexts are not emitted as symbols
    fn test_symbol_nested_assignments() {
        insta::assert_debug_snapshot!(test_symbol(
            "
local({
  inner1 <- 1            # Not a symbol
})
a <- function() {
  inner2 <- 2            # Not a symbol
  inner3 <- function() 3 # Symbol
}
"
        ));
        assert_eq!(test_symbol("{ foo <- 1 }"), vec![]);
    }

    #[test]
    fn test_symbol_nested_assignments_enabled() {
        let doc = Document::new(
            "
local({
  inner1 <- 1
})
a <- function() {
  inner2 <- 2
  inner3 <- function() 3
}
",
            None,
        );
        let node = doc.ast.root_node();

        let ctx = &mut CollectContext::new();
        ctx.include_assignments_in_blocks = true;

        let mut symbols = Vec::new();
        collect_symbols(ctx, &node, &doc.contents, 0, &mut symbols).unwrap();

        insta::assert_debug_snapshot!(symbols);
    }

    #[test]
    fn test_workspace_symbols_include_comment_sections() {
        fn run(include_comment_sections: bool) -> Vec<String> {
            let _guard = IndexerGuard;

            let code = "# Section ----\nfoo <- 1";

            let mut config = LspConfig::default();
            config.workspace_symbols = WorkspaceSymbolsConfig {
                include_comment_sections,
            };
            let mut state = WorldState::default();
            state.config = config;

            // Index the document
            let doc = Document::new(code, None);
            indexer::update(&doc, std::path::Path::new("/test.R")).unwrap();

            // Query for all symbols
            let params = WorkspaceSymbolParams {
                query: "Section".to_string(),
                ..Default::default()
            };
            let result = super::symbols(&params, &state).unwrap();
            let out = result.into_iter().map(|s| s.name).collect();

            out
        }

        // Should include section when true
        let with_sections = run(true);
        assert!(with_sections.contains(&"Section".to_string()));

        // Should not include section when false
        let without_sections = run(false);
        assert!(!without_sections.contains(&"Section".to_string()));
    }
}
