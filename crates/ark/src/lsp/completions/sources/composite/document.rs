//
// document.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::Result;
use stdext::*;
use tower_lsp::lsp_types::CompletionItem;
use tree_sitter::Node;

use crate::lsp::completions::completion_item::completion_item_from_assignment;
use crate::lsp::completions::completion_item::completion_item_from_scope_parameter;
use crate::lsp::completions::sources::utils::filter_out_dot_prefixes;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::traits::cursor::TreeCursorExt;
use crate::lsp::traits::node::NodeExt;
use crate::lsp::traits::point::PointExt;
use crate::lsp::traits::rope::RopeExt;

pub(super) fn completions_from_document(
    context: &DocumentContext,
) -> Result<Option<Vec<CompletionItem>>> {
    // get reference to AST
    let mut node = context.node;

    if node.is_comment() {
        log::error!("Should have been handled by comment completion source.");
        return Ok(None);
    }
    if matches!(node.kind(), "::" | ":::" | "$" | "[" | "[[") {
        log::error!("Should have been handled by alternative completion source.");
        return Ok(None);
    }

    let mut completions = vec![];

    loop {
        // If this is a brace list, or the document root, recurse to find identifiers.
        if node.kind() == "{" || node.parent() == None {
            completions.append(&mut completions_from_document_variables(&node, context));
        }

        // If this is a function definition, add parameter names.
        if node.kind() == "function" {
            completions.append(&mut completions_from_document_function_arguments(
                &node, context,
            )?);
        }

        // Keep going.
        node = match node.parent() {
            Some(node) => node,
            None => break,
        };
    }

    // Assume that even if they are in the document, we still don't want
    // to include them without explicit user request
    filter_out_dot_prefixes(context, &mut completions);

    Ok(Some(completions))
}

fn completions_from_document_variables(
    node: &Node,
    context: &DocumentContext,
) -> Vec<CompletionItem> {
    let mut completions = vec![];

    let mut cursor = node.walk();

    cursor.recurse(|node| {
        // skip nodes that exist beyond the completion position
        if node.start_position().is_after(context.point) {
            return false;
        }

        match node.kind() {
            "=" | "<-" | "<<-" => {
                // check that the left-hand side is an identifier or a string
                if let Some(child) = node.child(0) {
                    if child.is_identifier_or_string() {
                        match completion_item_from_assignment(&node, context) {
                            Ok(item) => completions.push(item),
                            Err(err) => log::error!("{err:?}"),
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

    completions
}

fn completions_from_document_function_arguments(
    node: &Node,
    context: &DocumentContext,
) -> Result<Vec<CompletionItem>> {
    let mut completions = vec![];

    // get the parameters node
    let parameters = node.child_by_field_name("parameters").into_result()?;

    let mut cursor = parameters.walk();

    // iterate through the children, looking for parameters with known names
    for node in parameters.children(&mut cursor) {
        if node.kind() != "parameter" {
            continue;
        }

        let node = unwrap!(node.child_by_field_name("name"), None => {
            continue;
        });

        if !node.is_identifier() {
            continue;
        }

        let parameter = context.document.contents.node_slice(&node)?.to_string();
        match completion_item_from_scope_parameter(parameter.as_str(), context) {
            Ok(item) => completions.push(item),
            Err(err) => log::error!("{err:?}"),
        }
    }

    Ok(completions)
}

fn call_uses_nse(node: &Node, context: &DocumentContext) -> bool {
    let result: Result<()> = local! {

        let lhs = node.child(0).into_result()?;
        lhs.is_identifier_or_string().into_result()?;

        let value = context.document.contents.node_slice(&lhs)?.to_string();
        matches!(value.as_str(), "expression" | "local" | "quote" | "enquote" | "substitute" | "with" | "within").into_result()?;

        Ok(())
    };

    result.is_ok()
}
