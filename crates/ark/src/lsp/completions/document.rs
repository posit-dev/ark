//
// document.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::collections::HashSet;

use anyhow::Result;
use log::*;
use stdext::*;
use tower_lsp::lsp_types::CompletionItem;
use tree_sitter::Node;

use crate::lsp::completions::completion_item::completion_item_from_assignment;
use crate::lsp::completions::completion_item::completion_item_from_scope_parameter;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::traits::cursor::TreeCursorExt;
use crate::lsp::traits::point::PointExt;

pub fn append_document_completions(
    context: &DocumentContext,
    completions: &mut Vec<CompletionItem>,
) -> Result<()> {
    // get reference to AST
    let mut node = context.node;

    // skip comments
    if node.kind() == "comment" {
        trace!("cursor position lies within R comment; not providing document completions");
        return Ok(());
    }

    // don't complete following subset-style operators
    if matches!(node.kind(), "::" | ":::" | "$" | "[" | "[[") {
        return Ok(());
    }

    let mut visited: HashSet<usize> = HashSet::new();
    loop {
        // If this is a brace list, or the document root, recurse to find identifiers.
        if node.kind() == "{" || node.parent() == None {
            append_defined_variables(&node, context, completions);
        }

        // If this is a function definition, add parameter names.
        if node.kind() == "function" {
            let result = append_function_parameters(&node, context, completions);
            if let Err(error) = result {
                error!("{:?}", error);
            }
        }

        // Mark this node as visited.
        visited.insert(node.id());

        // Keep going.
        node = match node.parent() {
            Some(node) => node,
            None => break,
        };
    }

    Ok(())
}

fn append_defined_variables(
    node: &Node,
    context: &DocumentContext,
    completions: &mut Vec<CompletionItem>,
) {
    let visited: HashSet<usize> = HashSet::new();

    let mut cursor = node.walk();
    cursor.recurse(|node| {
        // skip nodes that exist beyond the completion position
        if node.start_position().is_after(context.point) {
            return false;
        }

        // skip nodes that were already visited
        if visited.contains(&node.id()) {
            return false;
        }

        match node.kind() {
            "=" | "<-" | "<<-" => {
                // check that the left-hand side is an identifier or a string
                if let Some(child) = node.child(0) {
                    if matches!(child.kind(), "identifier" | "string") {
                        match completion_item_from_assignment(&node, context) {
                            Ok(item) => completions.push(item),
                            Err(error) => error!("{:?}", error),
                        }
                    }
                }

                // return true in case we have nested assignments
                return true;
            },

            "->" | "->>" => {
                // return true for nested assignments
                return true;
            },

            "call" => {
                // don't recurse into calls for certain functions
                return !call_uses_nse(&node, context);
            },

            "function" => {
                // don't recurse into function definitions, as these create as new scope
                // for variable definitions (and so such definitions are no longer visible)
                return false;
            },

            _ => {
                return true;
            },
        }
    });
}

// TODO: Pick a name that makes it clear this is a function defined in the associated document.
fn append_function_parameters(
    node: &Node,
    context: &DocumentContext,
    completions: &mut Vec<CompletionItem>,
) -> Result<()> {
    // get the parameters node
    let parameters = node.child_by_field_name("parameters").into_result()?;

    // iterate through the children, looking for parameters with known names
    let mut cursor = parameters.walk();
    for node in parameters.children(&mut cursor) {
        if node.kind() != "parameter" {
            continue;
        }

        let node = unwrap!(node.child_by_field_name("name"), None => {
            continue;
        });

        if node.kind() != "identifier" {
            continue;
        }

        let parameter = node.utf8_text(context.source.as_bytes()).into_result()?;
        match completion_item_from_scope_parameter(parameter, context) {
            Ok(item) => completions.push(item),
            Err(error) => error!("{:?}", error),
        }
    }

    Ok(())
}

fn call_uses_nse(node: &Node, context: &DocumentContext) -> bool {
    let result: Result<()> = local! {

        let lhs = node.child(0).into_result()?;
        matches!(lhs.kind(), "identifier" | "string").into_result()?;

        let value = lhs.utf8_text(context.source.as_bytes())?;
        matches!(value, "expression" | "local" | "quote" | "enquote" | "substitute" | "with" | "within").into_result()?;

        Ok(())

    };

    result.is_ok()
}
