//
// call_arguments.rs
//
// Copyright (C) 2022-2026 Posit Software, PBC. All rights reserved.
//
//

use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_db::Db;
use oak_db::File;
use oak_ide::callee_description;
use oak_ide::CalleeDescription;
use tree_sitter::Node;
use tree_sitter::Point;

use crate::lsp::document_context::DocumentContext;
use crate::lsp::traits::node::NodeExt;
use crate::lsp::traits::point::PointExt;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

/// The arguments of the call around the cursor, as written in the document.
///
/// R calls can mix named and unnamed arguments, as in
/// `foo(a = 1, b, c = 2, d)`, so comma count alone does not identify the active
/// parameter. This records the input for [active_parameter()]'s approximation
/// of argument matching.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CallArguments {
    /// Text of the callee, e.g. `f` in `f(x)` or `pkg::f` in `pkg::f(x)`.
    pub(crate) callee: String,
    /// Names of the arguments supplied as `name = value`.
    pub(crate) explicit_names: Vec<String>,
    /// One per comma up to the node under the cursor, minus one per named
    /// argument anywhere in the call, including those after the cursor. This
    /// can be negative.
    pub(crate) unnamed_count: i32,
    /// Name of the argument under the cursor when supplied as `name = value`.
    pub(crate) active_name: Option<String>,
}

/// A call found by [call_at()], retaining the callee node for syntax-based
/// lookups such as namespace detection in signature help.
pub(crate) struct CallAt<'tree> {
    pub(crate) callee: Node<'tree>,
    pub(crate) arguments: CallArguments,
}

/// Collects arguments of the enclosing call, or returns `None` when the cursor
/// is outside its parentheses.
pub(crate) fn call_arguments_at(
    context: &DocumentContext,
) -> anyhow::Result<Option<CallArguments>> {
    Ok(call_at(context)?.map(|call| call.arguments))
}

/// Like [call_arguments_at()], also returning the callee node.
pub(crate) fn call_at<'tree>(
    context: &DocumentContext<'tree>,
) -> anyhow::Result<Option<CallAt<'tree>>> {
    // Find the node closest to the completion point.
    let node = context.tree.root_node();
    let Some(mut node) = node.find_closest_node_to_point(context.point) else {
        return Ok(None);
    };

    // If we landed on a comma before the cursor position, move to the next sibling node.
    // We need to check the position as, if the cursor is "on" the comma as in
    //
    //    foo (x = ,)
    //
    // then the current context is associated with 'x = ' and not with what follows
    // the comma.
    if node.node_type() == NodeType::Comma && node.start_position().is_before(context.point) {
        if let Some(sibling) = node.next_sibling() {
            node = sibling;
        }
    }

    if node.node_type() == NodeType::Anonymous(String::from(")")) {
        if let Some(sibling) = node.prev_sibling() {
            node = sibling;
        }
    }

    // Get the current node.
    let Some(mut parent) = node.parent() else {
        return Ok(None);
    };

    // The list of arguments that have been explicitly specified.
    let mut explicit_names = vec![];

    // The number of unnamed arguments that have been supplied.
    let mut unnamed_count = 0;

    // The active argument, if any. Relevant for cases where the cursor is lying after 'x = <...>',
    // so we know that 'x' must be active.
    let mut active_name = None;

    // Whether we've found the child node we were looking for.
    let mut found_child = false;

    let call = loop {
        // If we found an 'arguments' node, then use that to infer the current offset.
        if parent.node_type() == NodeType::Arguments {
            // If the cursor lies upon a named argument, use that as an override.
            if let Some(name) = node.child_by_field_name("name") {
                let name = name.node_to_string(context.contents)?;
                active_name = Some(name);
            }

            let mut cursor = parent.walk();
            let children = parent.children(&mut cursor);
            for child in children {
                if let Some(name) = child.child_by_field_name("name") {
                    // If this is a named argument, add it to the list.
                    let name = name.node_to_string(context.contents)?;
                    explicit_names.push(name);

                    // Subtract 1 from the number of unnamed arguments, as
                    // the next comma we see won't be associated with an
                    // unnamed argument.
                    unnamed_count -= 1;
                }

                // If we find a comma, add to the offset.
                if !found_child && child.node_type() == NodeType::Comma {
                    unnamed_count += 1;
                }

                // If we've now walked up to the current node, we can quit.
                if child == node {
                    found_child = true;
                }
            }
        }

        // If we find the 'call' node, we can quit.
        if parent.is_call() {
            break parent;
        }

        // Update.
        node = parent;
        let Some(next) = node.parent() else {
            return Ok(None);
        };
        parent = next;
    };

    // `find_closest_node_to_point()` can select a call entirely before the
    // cursor. Only accept a call when the cursor is inside its parentheses.
    if !is_within_call_parentheses(&context.point, &call) {
        return Ok(None);
    }

    // Get the left-hand side of the call.
    let Some(callee) = call.child(0) else {
        return Ok(None);
    };

    let arguments = CallArguments {
        callee: callee.node_to_string(context.contents)?,
        explicit_names,
        unnamed_count,
        active_name,
    };

    Ok(Some(CallAt { callee, arguments }))
}

/// Adapts a tree-sitter callee to Oak's static lookup. `callee` must come
/// from `file`'s current source text so its byte range also identifies the
/// expression in Oak's syntax tree.
pub(crate) fn describe_callee<'db>(
    db: &'db dyn Db,
    file: File,
    callee: &Node,
) -> anyhow::Result<Option<CalleeDescription<'db>>> {
    let start = TextSize::from(u32::try_from(callee.start_byte())?);
    let end = TextSize::from(u32::try_from(callee.end_byte())?);
    Ok(callee_description(db, file, TextRange::new(start, end)).description)
}

/// Approximates the index of the active formal parameter.
///
/// An exact name match takes priority. Otherwise, skips formals supplied by
/// name and uses the positive part of `unnamed_count` as a positional offset.
/// An unknown active name also falls back to this positional lookup.
pub(crate) fn active_parameter(arguments: &CallArguments, formals: &[String]) -> Option<usize> {
    if let Some(active_name) = &arguments.active_name {
        if let Some(index) = formals.iter().position(|formal| formal == active_name) {
            return Some(index);
        }
    }

    let mut unnamed_count = arguments.unnamed_count;

    for (index, formal) in formals.iter().enumerate() {
        // Was this argument explicitly provided? If so, skip it.
        if arguments.explicit_names.contains(formal) {
            continue;
        }

        // Otherwise, check and see if we have any remaining commas.
        if unnamed_count > 0 {
            unnamed_count -= 1;
            continue;
        }

        return Some(index);
    }

    None
}

fn is_within_call_parentheses(x: &Point, node: &Node) -> bool {
    if node.node_type() != NodeType::Call {
        // This would be very weird
        log::error!("`is_within_call_parentheses()` called on a non-`call` node.");
        return false;
    }

    let Some(arguments) = node.child_by_field_name("arguments") else {
        return false;
    };

    let Some(open) = arguments.child_by_field_name("open") else {
        return false;
    };
    let Some(close) = arguments.child_by_field_name("close") else {
        return false;
    };

    x.is_after_or_equal(open.end_position()) && x.is_before_or_equal(close.start_position())
}

#[cfg(test)]
mod tests {
    use crate::fixtures::point_from_cursor;
    use crate::lsp::call_arguments::active_parameter;
    use crate::lsp::call_arguments::call_arguments_at;
    use crate::lsp::call_arguments::CallArguments;
    use crate::lsp::document_context::TestDocument;

    fn arguments_at_cursor(code: &str) -> Option<CallArguments> {
        let (text, point) = point_from_cursor(code);
        let doc = TestDocument::new(&text);
        let context = doc.context(point);
        call_arguments_at(&context).unwrap()
    }

    fn formals(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn test_call_arguments_at() {
        // The comma immediately after the cursor counts toward `unnamed_count`.
        assert_eq!(
            arguments_at_cursor("f(a, b = 1, @, d = 2)"),
            Some(CallArguments {
                callee: String::from("f"),
                explicit_names: vec![String::from("b"), String::from("d")],
                unnamed_count: 1,
                active_name: None,
            })
        );

        assert_eq!(
            arguments_at_cursor("pkg::f(x = @)"),
            Some(CallArguments {
                callee: String::from("pkg::f"),
                explicit_names: vec![String::from("x")],
                unnamed_count: -1,
                active_name: Some(String::from("x")),
            })
        );
    }

    #[test]
    fn test_call_arguments_outside_parentheses() {
        assert_eq!(arguments_at_cursor("f@()"), None);
        assert_eq!(arguments_at_cursor("f()@"), None);
        assert_eq!(arguments_at_cursor("x@"), None);
    }

    #[test]
    fn test_active_parameter() {
        let formals = formals(&["a", "b", "c", "d"]);

        // `b` is excluded from positional matching even after the cursor.
        let arguments = arguments_at_cursor("f(1, @, b = 2)").unwrap();
        assert_eq!(active_parameter(&arguments, &formals), Some(2));

        let arguments = arguments_at_cursor("f(1, c = @)").unwrap();
        assert_eq!(active_parameter(&arguments, &formals), Some(2));

        let arguments = arguments_at_cursor("f(1, 2, 3, 4, @)").unwrap();
        assert_eq!(active_parameter(&arguments, &formals), None);

        // An unknown active name falls back to positional matching.
        let arguments = arguments_at_cursor("f(zzz = @)").unwrap();
        assert_eq!(active_parameter(&arguments, &formals), Some(0));
    }
}
