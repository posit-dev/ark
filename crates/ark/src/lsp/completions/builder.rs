use anyhow::Result;
use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::completions::parameter_hints::ParameterHints;
use crate::lsp::completions::parameter_hints::{self};
use crate::lsp::completions::sources::composite::completions_from_composite_sources;
use crate::lsp::completions::sources::unique::colon::completions_from_single_colon;
use crate::lsp::completions::sources::unique::comment::completions_from_comment;
use crate::lsp::completions::sources::unique::custom::completions_from_custom_source;
use crate::lsp::completions::sources::unique::extractor::completions_from_at;
use crate::lsp::completions::sources::unique::extractor::completions_from_dollar;
use crate::lsp::completions::sources::unique::namespace::completions_from_namespace;
use crate::lsp::completions::sources::unique::string::completions_from_string;
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
        // Try unique sources first
        if let Some(completions) = self.completions_from_unique_sources()? {
            return Ok(completions);
        }

        // At this point we aren't in a "unique" completion case, so just return a
        // set of reasonable completions based on loaded packages, the open
        // document, the current workspace, and any call related arguments
        completions_from_composite_sources(self.context, self.state, self.parameter_hints)
    }

    pub fn completions_from_unique_sources(&self) -> Result<Option<Vec<CompletionItem>>> {
        // Try to detect a single colon first, which is a special case where we
        // don't provide any completions
        if let Some(completions) = completions_from_single_colon(self.context)? {
            return Ok(Some(completions));
        }

        if let Some(completions) = completions_from_comment(self.context)? {
            return Ok(Some(completions));
        }

        if let Some(completions) = completions_from_string(self.context)? {
            return Ok(Some(completions));
        }

        if let Some(completions) = completions_from_namespace(self.context, self.parameter_hints)? {
            return Ok(Some(completions));
        }

        if let Some(completions) = completions_from_custom_source(self.context)? {
            return Ok(Some(completions));
        }

        if let Some(completions) = completions_from_dollar(self.context)? {
            return Ok(Some(completions));
        }

        if let Some(completions) = completions_from_at(self.context)? {
            return Ok(Some(completions));
        }

        // No unique sources of completions, allow composite sources to run
        Ok(None)
    }
}
