//
// builder.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
//
//

use std::cell::OnceCell;

use anyhow::Result;
use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::completions::parameter_hints::ParameterHints;
use crate::lsp::completions::parameter_hints::{self};
use crate::lsp::completions::sources::composite::find_pipe_root;
use crate::lsp::completions::sources::composite::pipe::PipeRoot;
use crate::lsp::completions::sources::composite::CompositeCompletionsSource;
use crate::lsp::completions::sources::unique::UniqueCompletionsSource;
use crate::lsp::completions::sources::CompletionSource;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::state::WorldState;

pub(crate) struct CompletionBuilder<'a> {
    pub(crate) context: &'a DocumentContext<'a>,
    pub(crate) state: &'a WorldState,
    pub(crate) parameter_hints: ParameterHints,
    pipe_root: OnceCell<Option<PipeRoot>>,
}

impl<'a> CompletionBuilder<'a> {
    pub fn new(context: &'a DocumentContext, state: &'a WorldState) -> Self {
        let parameter_hints =
            parameter_hints::parameter_hints(context.node, &context.document.contents);
        Self {
            context,
            state,
            parameter_hints,
            pipe_root: OnceCell::new(),
        }
    }

    pub fn get_pipe_root(&self) -> Result<Option<PipeRoot>> {
        if let Some(root) = self.pipe_root.get() {
            // Already computed, just clone and return
            return Ok(root.clone());
        }

        // Not yet computed, find the pipe root
        let root = find_pipe_root(self.context)?;

        // Cache it for future calls (ignore failure if race condition, which shouldn't happen)
        let _ = self.pipe_root.set(root.clone());

        // Return the result
        Ok(root)
    }

    pub fn build(self) -> Result<Vec<CompletionItem>> {
        // Try unique sources first
        let unique_sources = UniqueCompletionsSource;
        if let Some(completions) = unique_sources.provide_completions(&self)? {
            return Ok(completions);
        }

        // At this point we aren't in a "unique" completion case, so just return a
        // set of reasonable completions from composite sources
        let composite_sources = CompositeCompletionsSource;
        Ok(composite_sources
            .provide_completions(&self)?
            .unwrap_or_default())
    }
}
