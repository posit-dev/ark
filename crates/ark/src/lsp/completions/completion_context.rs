//
// completion_context.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
//
//

use std::cell::OnceCell;

use tree_sitter::Node;

use crate::lsp::completions::function_context::FunctionContext;
use crate::lsp::completions::sources::composite::pipe::find_pipe_root;
use crate::lsp::completions::sources::composite::pipe::PipeRoot;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::state::WorldState;
use crate::treesitter::node_find_containing_call;
pub(crate) struct CompletionContext<'a> {
    pub(crate) document_context: &'a DocumentContext<'a>,
    pub(crate) state: &'a WorldState,
    pipe_root_cell: OnceCell<Option<PipeRoot>>,
    containing_call_cell: OnceCell<Option<Node<'a>>>,
    function_context_cell: OnceCell<anyhow::Result<FunctionContext>>,
}

impl<'a> CompletionContext<'a> {
    pub fn new(document_context: &'a DocumentContext, state: &'a WorldState) -> Self {
        Self {
            document_context,
            state,
            pipe_root_cell: OnceCell::new(),
            containing_call_cell: OnceCell::new(),
            function_context_cell: OnceCell::new(),
        }
    }

    pub fn pipe_root(&self) -> Option<PipeRoot> {
        let call_node = self.containing_call_node();

        self.pipe_root_cell
            .get_or_init(|| match find_pipe_root(self.document_context, call_node) {
                Ok(root) => root,
                Err(e) => {
                    log::trace!("Error trying to find pipe root: {e}");
                    None
                },
            })
            .clone()
    }

    pub fn containing_call_node(&self) -> Option<Node<'a>> {
        *self
            .containing_call_cell
            .get_or_init(|| node_find_containing_call(self.document_context.node))
    }

    pub fn function_context(&self) -> anyhow::Result<&FunctionContext> {
        self.function_context_cell
            .get_or_init(|| FunctionContext::new(&self.document_context))
            .as_ref()
            .map_err(|err| anyhow::anyhow!("{err:?}"))
    }
}
