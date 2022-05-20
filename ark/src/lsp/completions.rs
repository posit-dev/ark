// 
// completions.rs
// 
// Copyright (C) 2022 by RStudio, PBC
// 
// 

use tower_lsp::lsp_types::CompletionItem;
use tower_lsp::lsp_types::CompletionParams;
use tree_sitter::Node;
use tree_sitter::Point;

use crate::lsp::document::Document;
use crate::lsp::logger::log_push;
use crate::lsp::macros::unwrap;
use crate::lsp::traits::cursor::TreeCursorExt;
use crate::lsp::traits::node::NodeExt;
use crate::lsp::traits::point::PointExt;
use crate::lsp::traits::position::PositionExt;

fn completion_from_identifier(node: &Node, source: &str) -> CompletionItem {
    let label = node.utf8_text(source.as_bytes()).expect("empty assignee");
    let detail = format!("Defined on line {}", node.start_position().row + 1);
    CompletionItem::new_simple(label.to_string(), detail)
}

fn append_defined_variables(node: &Node, source: &str, end: Option<Point>, completions: &mut Vec<CompletionItem>) {

    log_push!("append_defined_variables(): Dumping AST. {}", node.dump(source));
    let mut cursor = node.walk();
    cursor.recurse(|node| {

        // skip nodes that exist beyond the completion position
        if let Some(end) = end {
            if node.start_position().is_after(end) {
                // log_push!("append_defined_variables(): Halting recursion after point {}.", end);
                return false;
            }
        }

        // log_push!("append_defined_variables(): {:#?}", node);
        match node.kind() {

            "left_assignment" | "super_assignment" | "equals_assignment" => {

                // TODO: Should we de-quote symbols and strings, or insert them as-is?
                let assignee = node.child(0).unwrap();
                if assignee.kind() == "identifier" || assignee.kind() == "string" {
                    completions.push(completion_from_identifier(&assignee, &source));
                }

                // return true in case we have nested assignments
                return true;

            }

            "right_assignment" | "super_right_assignment" => {

                // return true for nested assignments
                return true;

            }

            "function_definition" => {

                // don't recurse into function definitions, as these create as new scope
                // for variable definitions (and so such definitions are no longer visible)
                // log_push!("append_defined_variables(): Halting recursion (found 'function_definition').");
                return false;

            }

            _ => {
                return true;
            }

        }

    });

}

fn append_function_parameters(node: &Node, source: &str, completions: &mut Vec<CompletionItem>) {

    let mut cursor = node.walk();
    
    if !cursor.goto_first_child() {
        // log_push!("append_function_completions(): goto_first_child() failed");
        return;
    }

    if !cursor.goto_next_sibling() {
        // log_push!("append_function_completions(): goto_next_sibling() failed");
        return;
    }

    let kind = cursor.node().kind();
    if kind != "formal_parameters" {
        // log_push!("append_function_completions(): unexpected node kind {}", kind);
        return;
    }

    if !cursor.goto_first_child() {
        // log_push!("append_function_completions(): goto_first_child() failed");
        return;
    }

    // The R tree-sitter grammar doesn't parse an R function's formals list into
    // a tree; instead, it's just held as a sequence of tokens. that said, the
    // only way an identifier could / should show up here is if it is indeed a
    // function parameter, so just search direct children here for identifiers.
    while cursor.goto_next_sibling() {
        let node = cursor.node();
        if node.kind() == "identifier" {
            completions.push(completion_from_identifier(&node, &source));
        }
    }

}


pub(crate) fn append_document_completions(document: &mut Document, params: &CompletionParams, completions: &mut Vec<CompletionItem>) {

    let ast = unwrap!(&mut document.ast, {
        // log_push!("append_completions(): No AST available.");
        return;
    });

    let point = params.text_document_position.position.as_point();
    let mut node = unwrap!(ast.root_node().descendant_for_point_range(point, point), {
        // log_push!("append_completions(): Couldn't find node for point {}", point);
        return;
    });

    // log_push!("append_completions(): Found node {:?} at [{}, {}]", node, point.row, point.column);

    let source = document.contents.to_string();
    let mut end = Some(point);

    loop {

        // If this is a brace list, or the document root, recurse to find identifiers.
        if node.kind() == "brace_list" || node.parent() == None {
            // log_push!("append_defined_variables(): Entering scope. ({:?})", node);
            append_defined_variables(&node, &source, end, completions);
            end = None;
        }

        // If this is a function definition, add parameter names.
        if node.kind() == "function_definition" {
            // log_push!("append_defined_variables(): Adding function parameters. ({:?})", node);
            append_function_parameters(&node, &source, completions);
        }

        // Keep going.
        node = match node.parent() {
            Some(node) => node,
            None => break,
        };

    }

    // walk(&mut cursor, |node| {

    //     // check for assignments
    //     if node.kind() == "left_assignment" && node.child_count() > 0 {
    //         let lhs = node.child(0).unwrap();
    //         if lhs.kind() == "identifier" {
    //             let variable = lhs.utf8_text(contents.as_bytes());
    //             if let Ok(variable) = variable {
    //                 let detail = format!("Defined on row {}", node.range().start_point.row + 1);
    //                 completions.push(CompletionItem::new_simple(variable.to_string(), detail));
    //             }
    //         }
    //     }

    //     return true;

    // });

}
