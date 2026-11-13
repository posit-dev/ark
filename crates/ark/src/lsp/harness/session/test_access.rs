//! Session access that only tests need, such as custom source handlers and
//! tasks spawned straight onto the analysis pool. Benchmarks stay on the public
//! session API.

use std::sync::Arc;

use serde_json::Value;

use super::LspSession;
use crate::lsp::analysis::WorldStateSnapshot;
use crate::lsp::handlers::Handlers;
use crate::lsp::harness::LspHarness;
use crate::lsp::harness::TestHandlers;
use crate::lsp::main_loop::DiagnosticsPublication;
use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::TokioUnboundedSender;
use crate::lsp::sources::SourceHandler;
use crate::lsp::state::WorldState;

impl LspHarness {
    pub(crate) async fn start_with_sources(
        self,
        settings: &[(&str, Value)],
        source_handler: Arc<dyn SourceHandler>,
    ) -> LspSession {
        self.start_with(TestHandlers, settings, Some(source_handler))
            .await
    }

    /// [`Self::start()`] with a host's `handlers` in place of [`TestHandlers`].
    pub(crate) async fn start_with_handlers<H: Handlers>(
        self,
        handlers: H,
        settings: &[(&str, Value)],
    ) -> LspSession<H> {
        self.start_with(handlers, settings, None).await
    }
}

impl<H: Handlers> LspSession<H> {
    pub(crate) async fn pump_scans_to_quiescence(&mut self) {
        self.state.pump_scans_to_quiescence().await;
        self.collect_auxiliary();
    }

    pub(crate) fn world(&self) -> &WorldState {
        self.state.world()
    }

    pub(crate) fn raw_publications(&self) -> &[DiagnosticsPublication] {
        &self.raw_publications
    }

    pub(crate) fn events_tx(&self) -> TokioUnboundedSender<Event<H>> {
        self.state.events_tx()
    }

    /// Run `task` on the analysis pool without going through a scheduler. Tests
    /// use this to create worker states that normal requests cannot hold, such
    /// as sending an [`Event`] and then blocking before the task returns.
    pub(crate) fn spawn_analysis_task(
        &self,
        task: impl FnOnce(WorldStateSnapshot) + Send + 'static,
    ) {
        self.state
            .lsp_state()
            .analysis_pool
            .spawn_test_task(self.state.world().snapshot(), task);
    }
}
