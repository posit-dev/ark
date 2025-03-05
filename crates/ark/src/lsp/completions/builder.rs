use anyhow::Result;
use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::completions::parameter_hints::ParameterHints;
use crate::lsp::completions::parameter_hints::{self};
use crate::lsp::completions::sources::completions_from_composite_sources;
use crate::lsp::completions::sources::completions_from_unique_sources;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::state::WorldState;

pub(crate) struct CompletionBuilder<'a> {
    context: &'a DocumentContext<'a>,
    state: &'a WorldState,
    parameter_hints: ParameterHints,
}

impl<'a> CompletionBuilder<'a> {
    pub fn new(context: &'a DocumentContext, state: &'a WorldState) -> Self {
        let parameter_hints =
            parameter_hints::parameter_hints(context.node, &context.document.contents);
        Self {
            context,
            state,
            parameter_hints,
        }
    }

    pub fn build(self) -> Result<Vec<CompletionItem>> {
        // Initially just delegate to existing functions
        if let Some(completions) =
            completions_from_unique_sources(self.context, self.parameter_hints)?
        {
            return Ok(completions);
        }

        // At this point we aren't in a "unique" completion case, so just return a
        // set of reasonable completions based on loaded packages, the open
        // document, the current workspace, and any call related arguments
        completions_from_composite_sources(self.context, self.state, self.parameter_hints)
    }
}
