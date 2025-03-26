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
    parameter_hints_cell: OnceCell<ParameterHints>,
    pipe_root_cell: OnceCell<Option<PipeRoot>>,
}

impl<'a> CompletionBuilder<'a> {
    pub fn new(context: &'a DocumentContext, state: &'a WorldState) -> Self {
        Self {
            context,
            state,
            parameter_hints_cell: OnceCell::new(),
            pipe_root_cell: OnceCell::new(),
        }
    }

    pub fn parameter_hints(&self) -> &ParameterHints {
        self.parameter_hints_cell.get_or_init(|| {
            parameter_hints::parameter_hints(self.context.node, &self.context.document.contents)
        })
    }

    pub fn pipe_root(&self) -> Result<Option<PipeRoot>> {
        if let Some(root) = self.pipe_root_cell.get() {
            return Ok(root.clone());
        }

        let root = find_pipe_root(self.context)?;

        // Cache it for future calls (ignore failure if race condition, which shouldn't happen)
        let _ = self.pipe_root_cell.set(root.clone());

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
