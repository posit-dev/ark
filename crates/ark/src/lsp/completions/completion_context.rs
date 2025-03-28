//
// completion_context.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
//
//

use std::cell::OnceCell;

use crate::lsp::completions::parameter_hints::ParameterHints;
use crate::lsp::completions::parameter_hints::{self};
use crate::lsp::completions::sources::composite::find_pipe_root;
use crate::lsp::completions::sources::composite::pipe::PipeRoot;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::state::WorldState;

pub(crate) struct CompletionContext<'a> {
    pub(crate) document_context: &'a DocumentContext<'a>,
    pub(crate) state: &'a WorldState,
    parameter_hints_cell: OnceCell<ParameterHints>,
    pipe_root_cell: OnceCell<Option<PipeRoot>>,
}

impl<'a> CompletionContext<'a> {
    pub fn new(document_context: &'a DocumentContext, state: &'a WorldState) -> Self {
        Self {
            document_context,
            state,
            parameter_hints_cell: OnceCell::new(),
            pipe_root_cell: OnceCell::new(),
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
                    log::error!("Error trying to find pipe root: {}", e);
                    None
                },
            })
            .clone()
    }
}
