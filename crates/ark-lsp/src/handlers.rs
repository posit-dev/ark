//
// handlers.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
//
//

use tower_lsp::lsp_types::*;
use tree_sitter::Node;
use tree_sitter::Point;

use crate::state::WorldState;

// ============================================================================
// Folding Range
// ============================================================================

pub fn folding_range(state: &WorldState, uri: &Url) -> Option<Vec<FoldingRange>> {
    let doc = state.get_document(uri)?;
    let tree = doc.tree.as_ref()?;
    let mut ranges = Vec::new();

    collect_folding_ranges(tree.root_node(), &mut ranges);

    Some(ranges)
}

fn collect_folding_ranges(node: Node, ranges: &mut Vec<FoldingRange>) {
    let kind = node.kind();

    // Fold braced expressions, function definitions, and control structures
    let should_fold = matches!(
        kind,
        "brace_list" | "function_definition" | "if_statement" | "for_statement" | "while_statement"
    );

    if should_fold && node.start_position().row != node.end_position().row {
        ranges.push(FoldingRange {
            start_line: node.start_position().row as u32,
            start_character: Some(node.start_position().column as u32),
            end_line: node.end_position().row as u32,
            end_character: Some(node.end_position().column as u32),
            kind: Some(FoldingRangeKind::Region),
            collapsed_text: None,
        });
    }

    // Recurse into children
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_folding_ranges(child, ranges);
    }
}

// ============================================================================
// Selection Range
// ============================================================================

pub fn selection_range(
    state: &WorldState,
    uri: &Url,
    positions: Vec<Position>,
) -> Option<Vec<SelectionRange>> {
    let doc = state.get_document(uri)?;
    let tree = doc.tree.as_ref()?;

    let mut results = Vec::new();
    for pos in positions {
        let point = Point::new(pos.line as usize, pos.character as usize);
        if let Some(range) = build_selection_range(tree.root_node(), point) {
            results.push(range);
        }
    }

    Some(results)
}

fn build_selection_range(root: Node, point: Point) -> Option<SelectionRange> {
    let mut node = root.descendant_for_point_range(point, point)?;
    let mut ranges: Vec<Range> = Vec::new();

    loop {
        let range = Range {
            start: Position::new(node.start_position().row as u32, node.start_position().column as u32),
            end: Position::new(node.end_position().row as u32, node.end_position().column as u32),
        };

        if ranges.last() != Some(&range) {
            ranges.push(range);
        }

        if let Some(parent) = node.parent() {
            node = parent;
        } else {
            break;
        }
    }

    // Build nested SelectionRange from innermost to outermost
    let mut result: Option<SelectionRange> = None;
    for range in ranges {
        result = Some(SelectionRange {
            range,
            parent: result.map(Box::new),
        });
    }

    result
}

// ============================================================================
// Document Symbols
// ============================================================================

pub fn document_symbol(state: &WorldState, uri: &Url) -> Option<DocumentSymbolResponse> {
    let doc = state.get_document(uri)?;
    let tree = doc.tree.as_ref()?;
    let text = doc.text();

    let mut symbols = Vec::new();
    collect_symbols(tree.root_node(), &text, &mut symbols);

    Some(DocumentSymbolResponse::Flat(symbols))
}

#[allow(deprecated)]
fn collect_symbols(node: Node, text: &str, symbols: &mut Vec<SymbolInformation>) {
    // Look for assignments: identifier <- value or identifier = value
    if node.kind() == "binary_operator" {
        let mut cursor = node.walk();
        let children: Vec<_> = node.children(&mut cursor).collect();

        if children.len() >= 3 {
            let lhs = children[0];
            let op = children[1];
            let rhs = children[2];

            let op_text = node_text(op, text);
            if matches!(op_text, "<-" | "=" | "<<-") && lhs.kind() == "identifier" {
                let name = node_text(lhs, text).to_string();
                let kind = if rhs.kind() == "function_definition" {
                    SymbolKind::FUNCTION
                } else {
                    SymbolKind::VARIABLE
                };

                symbols.push(SymbolInformation {
                    name,
                    kind,
                    tags: None,
                    deprecated: None,
                    location: Location {
                        uri: Url::parse("file:///").unwrap(), // Will be replaced
                        range: Range {
                            start: Position::new(
                                node.start_position().row as u32,
                                node.start_position().column as u32,
                            ),
                            end: Position::new(
                                node.end_position().row as u32,
                                node.end_position().column as u32,
                            ),
                        },
                    },
                    container_name: None,
                });
            }
        }
    }

    // Recurse
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_symbols(child, text, symbols);
    }
}

// ============================================================================
// Diagnostics
// ============================================================================

pub fn diagnostics(state: &WorldState, uri: &Url) -> Vec<Diagnostic> {
    let Some(doc) = state.get_document(uri) else {
        return Vec::new();
    };

    let Some(tree) = &doc.tree else {
        return Vec::new();
    };

    let mut diagnostics = Vec::new();

    // Collect syntax errors
    collect_syntax_errors(tree.root_node(), &mut diagnostics);

    diagnostics
}

fn collect_syntax_errors(node: Node, diagnostics: &mut Vec<Diagnostic>) {
    if node.is_error() || node.is_missing() {
        let message = if node.is_missing() {
            format!("Missing {}", node.kind())
        } else {
            "Syntax error".to_string()
        };

        diagnostics.push(Diagnostic {
            range: Range {
                start: Position::new(
                    node.start_position().row as u32,
                    node.start_position().column as u32,
                ),
                end: Position::new(
                    node.end_position().row as u32,
                    node.end_position().column as u32,
                ),
            },
            severity: Some(DiagnosticSeverity::ERROR),
            message,
            ..Default::default()
        });
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_syntax_errors(child, diagnostics);
    }
}

// ============================================================================
// Completions
// ============================================================================

pub fn completion(state: &WorldState, uri: &Url, position: Position) -> Option<CompletionResponse> {
    let doc = state.get_document(uri)?;
    let tree = doc.tree.as_ref()?;
    let text = doc.text();

    let point = Point::new(position.line as usize, position.character as usize);
    let node = tree.root_node().descendant_for_point_range(point, point)?;

    let mut items = Vec::new();

    // Check if we're in a namespace context (pkg::)
    if find_namespace_context(&node, &text).is_some() {
        // TODO: Get package exports from library
        return Some(CompletionResponse::Array(items));
    }

    // Add R keywords
    let keywords = [
        "if", "else", "repeat", "while", "function", "for", "in", "next", "break",
        "TRUE", "FALSE", "NULL", "Inf", "NaN", "NA", "NA_integer_", "NA_real_",
        "NA_complex_", "NA_character_", "library", "require", "return",
    ];

    for kw in keywords {
        items.push(CompletionItem {
            label: kw.to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            ..Default::default()
        });
    }

    // Add symbols from current document
    collect_document_completions(tree.root_node(), &text, &mut items);

    Some(CompletionResponse::Array(items))
}

fn find_namespace_context<'a>(node: &Node<'a>, text: &'a str) -> Option<&'a str> {
    // Walk up to find namespace_operator
    let mut current = *node;
    loop {
        if current.kind() == "namespace_operator" {
            let mut cursor = current.walk();
            let children: Vec<_> = current.children(&mut cursor).collect();
            if !children.is_empty() {
                return Some(node_text(children[0], text));
            }
        }
        current = current.parent()?;
    }
}

fn collect_document_completions(node: Node, text: &str, items: &mut Vec<CompletionItem>) {
    if node.kind() == "binary_operator" {
        let mut cursor = node.walk();
        let children: Vec<_> = node.children(&mut cursor).collect();

        if children.len() >= 3 {
            let lhs = children[0];
            let op = children[1];
            let rhs = children[2];

            let op_text = node_text(op, text);
            if matches!(op_text, "<-" | "=" | "<<-") && lhs.kind() == "identifier" {
                let name = node_text(lhs, text).to_string();
                let kind = if rhs.kind() == "function_definition" {
                    CompletionItemKind::FUNCTION
                } else {
                    CompletionItemKind::VARIABLE
                };

                items.push(CompletionItem {
                    label: name,
                    kind: Some(kind),
                    ..Default::default()
                });
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_document_completions(child, text, items);
    }
}

// ============================================================================
// Hover
// ============================================================================

pub fn hover(state: &WorldState, uri: &Url, position: Position) -> Option<Hover> {
    let doc = state.get_document(uri)?;
    let tree = doc.tree.as_ref()?;
    let text = doc.text();

    let point = Point::new(position.line as usize, position.character as usize);
    let node = tree.root_node().descendant_for_point_range(point, point)?;

    // For now, just show the node kind for identifiers
    if node.kind() == "identifier" {
        let name = node_text(node, &text);
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!("`{}`", name),
            }),
            range: Some(Range {
                start: Position::new(
                    node.start_position().row as u32,
                    node.start_position().column as u32,
                ),
                end: Position::new(
                    node.end_position().row as u32,
                    node.end_position().column as u32,
                ),
            }),
        });
    }

    None
}

// ============================================================================
// Signature Help
// ============================================================================

pub fn signature_help(
    state: &WorldState,
    uri: &Url,
    position: Position,
) -> Option<SignatureHelp> {
    let doc = state.get_document(uri)?;
    let tree = doc.tree.as_ref()?;
    let text = doc.text();

    let point = Point::new(position.line as usize, position.character as usize);

    // Find enclosing call
    let mut node = tree.root_node().descendant_for_point_range(point, point)?;

    loop {
        if node.kind() == "call" {
            let mut cursor = node.walk();
            let children: Vec<_> = node.children(&mut cursor).collect();

            if !children.is_empty() {
                let func_node = children[0];
                let func_name = node_text(func_node, &text);

                return Some(SignatureHelp {
                    signatures: vec![SignatureInformation {
                        label: format!("{}(...)", func_name),
                        documentation: None,
                        parameters: None,
                        active_parameter: None,
                    }],
                    active_signature: Some(0),
                    active_parameter: None,
                });
            }
        }

        node = node.parent()?;
    }
}

// ============================================================================
// Goto Definition
// ============================================================================

pub fn goto_definition(
    state: &WorldState,
    uri: &Url,
    position: Position,
) -> Option<GotoDefinitionResponse> {
    let doc = state.get_document(uri)?;
    let tree = doc.tree.as_ref()?;
    let text = doc.text();

    let point = Point::new(position.line as usize, position.character as usize);
    let node = tree.root_node().descendant_for_point_range(point, point)?;

    if node.kind() != "identifier" {
        return None;
    }

    let name = node_text(node, &text);

    // Search for definition in current document
    if let Some(def_range) = find_definition_in_tree(tree.root_node(), name, &text) {
        return Some(GotoDefinitionResponse::Scalar(Location {
            uri: uri.clone(),
            range: def_range,
        }));
    }

    None
}

fn find_definition_in_tree(node: Node, name: &str, text: &str) -> Option<Range> {
    if node.kind() == "binary_operator" {
        let mut cursor = node.walk();
        let children: Vec<_> = node.children(&mut cursor).collect();

        if children.len() >= 3 {
            let lhs = children[0];
            let op = children[1];

            let op_text = node_text(op, text);
            if matches!(op_text, "<-" | "=" | "<<-") && lhs.kind() == "identifier" {
                if node_text(lhs, text) == name {
                    return Some(Range {
                        start: Position::new(
                            lhs.start_position().row as u32,
                            lhs.start_position().column as u32,
                        ),
                        end: Position::new(
                            lhs.end_position().row as u32,
                            lhs.end_position().column as u32,
                        ),
                    });
                }
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(range) = find_definition_in_tree(child, name, text) {
            return Some(range);
        }
    }

    None
}

// ============================================================================
// On Type Formatting (Indentation)
// ============================================================================

pub fn on_type_formatting(
    state: &WorldState,
    uri: &Url,
    position: Position,
) -> Option<Vec<TextEdit>> {
    let doc = state.get_document(uri)?;
    let text = doc.text();

    // Simple indentation: match previous line's indentation
    if position.line == 0 {
        return None;
    }

    let prev_line_idx = position.line as usize - 1;
    let lines: Vec<&str> = text.lines().collect();

    if prev_line_idx >= lines.len() {
        return None;
    }

    let prev_line = lines[prev_line_idx];
    let indent: String = prev_line.chars().take_while(|c| c.is_whitespace()).collect();

    // Check if previous line ends with { or ( - add extra indent
    let trimmed = prev_line.trim_end();
    let extra_indent = if trimmed.ends_with('{') || trimmed.ends_with('(') {
        "  "
    } else {
        ""
    };

    let new_indent = format!("{}{}", indent, extra_indent);

    Some(vec![TextEdit {
        range: Range {
            start: Position::new(position.line, 0),
            end: Position::new(position.line, 0),
        },
        new_text: new_indent,
    }])
}

// ============================================================================
// Utilities
// ============================================================================

fn node_text<'a>(node: Node<'a>, text: &'a str) -> &'a str {
    &text[node.byte_range()]
}
