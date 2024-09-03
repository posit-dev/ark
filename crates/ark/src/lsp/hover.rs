//
// hover.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use anyhow::*;
use stdext::unwrap;
use stdext::unwrap::IntoResult;
use tower_lsp::lsp_types::MarkupContent;
use tower_lsp::lsp_types::MarkupKind;
use tree_sitter::Node;

use crate::lsp::document_context::DocumentContext;
use crate::lsp::help::RHtmlHelp;
use crate::lsp::traits::rope::RopeExt;
use crate::treesitter::NodeTypeExt;

enum HoverContext {
    Topic { topic: String },
    QualifiedTopic { package: String, topic: String },
}

fn hover_context(node: Node, context: &DocumentContext) -> Result<Option<HoverContext>> {
    // if the parent node is a namespace call, use that node instead
    // TODO: What if the user hovers the cursor over 'dplyr' in e.g. 'dplyr::mutate'?
    let mut node = node;
    if let Some(parent) = node.parent() {
        if parent.is_namespace_operator() {
            node = parent;
        }
    }

    // if we have a namespace call, use that to provide a qualified topic
    if node.is_namespace_operator() {
        let lhs = node.child_by_field_name("lhs").into_result()?;
        let rhs = node.child_by_field_name("rhs").into_result()?;

        let ok = lhs.is_identifier_or_string() && rhs.is_identifier_or_string();

        if !ok {
            return Ok(None);
        }

        let package = context.document.contents.node_slice(&lhs)?.to_string();
        let topic = context.document.contents.node_slice(&rhs)?.to_string();
        return Ok(Some(HoverContext::QualifiedTopic { package, topic }));
    }

    // otherwise, check for an identifier or a string
    if node.is_identifier_or_string() || node.is_keyword() {
        // only provide documentation for function calls for now,
        // since bare identifiers might not match the topic we expect
        if let Some(parent) = node.parent() {
            if !parent.is_call() {
                return Ok(None);
            }
        }

        // otherwise, use it
        let topic = context.document.contents.node_slice(&node)?.to_string();
        return Ok(Some(HoverContext::Topic { topic }));
    }

    Ok(None)
}

pub(crate) unsafe fn r_hover(context: &DocumentContext) -> anyhow::Result<Option<MarkupContent>> {
    // get the node
    let node = &context.node;

    // check for identifier
    if !node.is_identifier_or_string() && !node.is_keyword() {
        return Ok(None);
    }

    let ctx = hover_context(*node, context)?;
    let ctx = unwrap!(ctx, None => {
        return Ok(None);
    });

    // Currently, `hover_context()` restricts to only showing hover docs for functions,
    // so we also use `RHtmlHelp::new_function()` here
    let help = match ctx {
        HoverContext::QualifiedTopic { package, topic } => {
            RHtmlHelp::from_function(topic.as_str(), Some(package.as_str()))?
        },

        HoverContext::Topic { topic } => RHtmlHelp::from_function(topic.as_str(), None)?,
    };

    let help = unwrap!(help, None => {
        return Ok(None);
    });

    let markdown = help.markdown()?;
    Ok(Some(MarkupContent {
        kind: MarkupKind::Markdown,
        value: markdown,
    }))
}
