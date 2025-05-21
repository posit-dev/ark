//
// completion_context.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
//
//

use std::cell::OnceCell;

use crate::lsp::completions::parameter_hints::ParameterHints;
use crate::lsp::completions::parameter_hints::{self};
use crate::lsp::completions::sources::composite::pipe::find_pipe_root;
use crate::lsp::completions::sources::composite::pipe::PipeRoot;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::state::WorldState;
use crate::treesitter::NodeTypeExt;

pub(crate) struct CompletionContext<'a> {
    pub(crate) document_context: &'a DocumentContext<'a>,
    pub(crate) state: &'a WorldState,
    parameter_hints_cell: OnceCell<ParameterHints>,
    pipe_root_cell: OnceCell<Option<PipeRoot>>,
    is_in_call_cell: OnceCell<bool>,
}

impl<'a> CompletionContext<'a> {
    pub fn new(document_context: &'a DocumentContext, state: &'a WorldState) -> Self {
        Self {
            document_context,
            state,
            parameter_hints_cell: OnceCell::new(),
            pipe_root_cell: OnceCell::new(),
            is_in_call_cell: OnceCell::new(),
        }
    }

    pub fn parameter_hints(&self) -> &ParameterHints {
        self.parameter_hints_cell.get_or_init(|| {
            parameter_hints::parameter_hints(
                self.document_context.node,
                &self.document_context.document.contents,
            )
        })
    }

    pub fn pipe_root(&self) -> Option<PipeRoot> {
        self.pipe_root_cell
            .get_or_init(|| match find_pipe_root(self.document_context) {
                Ok(root) => root,
                Err(e) => {
                    log::error!("Error trying to find pipe root: {e}");
                    None
                },
            })
            .clone()
    }

    pub fn is_in_call(&self) -> &bool {
        self.is_in_call_cell.get_or_init(|| {
            let mut node = self.document_context.node;
            let mut found_call = false;

            loop {
                if node.is_call() {
                    found_call = true;
                    break;
                }

                // If we reach a brace list, stop searching
                if node.is_braced_expression() {
                    break;
                }

                // Update the node
                match node.parent() {
                    Some(parent) => node = parent,
                    None => break,
                };
            }

            found_call
        })
    }
}
