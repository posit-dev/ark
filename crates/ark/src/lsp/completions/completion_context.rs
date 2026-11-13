//
// completion_context.rs
//
// Copyright (C) 2025-2026 Posit Software, PBC. All rights reserved.
//
//

use std::cell::OnceCell;

use legacy_ide::treesitter::node_find_containing_call;
use oak_db::File;
use oak_lsp::state::WorldState;
use tree_sitter::Node;

use crate::lsp::completions::function_context::FunctionContext;
use crate::lsp::document_context::DocumentContext;
pub(crate) struct CompletionContext<'a> {
    pub(crate) document_context: &'a DocumentContext<'a>,
    /// The document being completed, in `state`'s database.
    pub(crate) file: File,
    pub(crate) state: &'a WorldState,
    containing_call_cell: OnceCell<Option<Node<'a>>>,
    function_context_cell: OnceCell<anyhow::Result<FunctionContext>>,
}

impl<'a> CompletionContext<'a> {
    pub fn new(document_context: &'a DocumentContext, file: File, state: &'a WorldState) -> Self {
        Self {
            document_context,
            file,
            state,
            containing_call_cell: OnceCell::new(),
            function_context_cell: OnceCell::new(),
        }
    }

    pub fn containing_call_node(&self) -> Option<Node<'a>> {
        *self
            .containing_call_cell
            .get_or_init(|| node_find_containing_call(self.document_context.node))
    }

    pub fn function_context(&self) -> anyhow::Result<&FunctionContext> {
        self.function_context_cell
            .get_or_init(|| FunctionContext::new(self.document_context))
            .as_ref()
            .map_err(|err| anyhow::anyhow!("{err:?}"))
    }
}
