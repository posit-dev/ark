//! A host for sessions and tests that need no host-specific behavior.

use std::convert::Infallible;

use tower_lsp_server::ls_types::ServerInfo;

use crate::backend::LspResult;
use crate::handlers::EventContext;
use crate::handlers::Handlers;
use crate::handlers::RequestContext;

/// A test host using the default request handlers. It accepts session scope
/// updates without requiring an R session and defines no custom requests.
#[derive(Debug, Default)]
pub struct TestHandlers;

#[derive(Debug)]
pub enum TestEvent {
    /// Supply names visible in the session without evaluating a command in R.
    SessionScopes(Vec<Vec<String>>),
}

impl Handlers for TestHandlers {
    type Request = Infallible;
    type Response = Infallible;
    type Event = TestEvent;

    fn server_info(&self) -> ServerInfo {
        ServerInfo {
            name: String::from("LSP test harness"),
            version: None,
        }
    }

    fn request(&self, request: Infallible, _cx: &RequestContext<'_>) -> LspResult<Infallible> {
        match request {}
    }

    fn event(&mut self, event: TestEvent, cx: &mut EventContext<'_>) -> anyhow::Result<()> {
        match event {
            TestEvent::SessionScopes(scopes) => cx.world.set_session_scopes(scopes),
        }
        Ok(())
    }
}
