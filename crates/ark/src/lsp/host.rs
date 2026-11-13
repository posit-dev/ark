//
// host.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

//! Inputs from Ark's R session and their application to the LSP's world state.

use crate::lsp::state::WorldState;

/// Information sent from the kernel to the LSP after each top-level evaluation.
#[derive(Debug)]
pub struct ConsoleInputs {
    /// List of console scopes, from innermost (global or debug) to outermost
    /// scope. Currently the scopes are vectors of symbol names. TODO: In the
    /// future, we should send structural information like search path, and let
    /// the LSP query us for the contents so that the LSP can cache the
    /// information.
    pub console_scopes: Vec<Vec<String>>,
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn did_change_console_inputs(
    inputs: ConsoleInputs,
    state: &mut WorldState,
) -> anyhow::Result<()> {
    // Diagnostics use session names, including those exposed by
    // `devtools::load_all()` during package development, so scope changes
    // require a refresh.
    state.set_session_scopes(inputs.console_scopes);

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use oak_db::OakDatabase;
    use tower_lsp_server::ls_types::Diagnostic;

    use super::ConsoleInputs;
    use crate::lsp::harness::LspHarness;
    use crate::lsp::harness::LspSession;
    use crate::lsp::main_loop::Event;
    use crate::lsp::main_loop::KernelNotification;

    /// Bounds every wait so a lost wakeup fails here rather than hanging to the
    /// harness timeout.
    const TIMEOUT: Duration = Duration::from_secs(10);

    /// Console inputs are not oak inputs, so `set_session_scopes()` must bump
    /// the revision to refresh diagnostics for an unchanged document. The
    /// diagnostic for `foo` disappears when the session defines it and returns
    /// when the session removes it.
    #[tokio::test]
    async fn test_console_inputs_refresh_diagnostics() {
        let workspace = tempfile::tempdir().unwrap();
        let script = workspace.path().join("script.R");
        let mut session = LspHarness::new(OakDatabase::new()).start(&[]).await;
        let undefined = vec![String::from("No symbol named 'foo' in scope.")];

        session.send_did_open(&script, "foo\n");
        assert_eq!(next_messages(&mut session, &script).await, undefined);

        session.enqueue(console_inputs(vec![vec![String::from("foo")]]));
        assert_eq!(
            next_messages(&mut session, &script).await,
            Vec::<String>::new()
        );

        session.enqueue(console_inputs(vec![]));
        assert_eq!(next_messages(&mut session, &script).await, undefined);
    }

    fn console_inputs(console_scopes: Vec<Vec<String>>) -> Event {
        Event::Kernel(KernelNotification::DidChangeConsoleInputs(ConsoleInputs {
            console_scopes,
        }))
    }

    /// Wait for accepted diagnostics for `path` at the version sent by `didOpen`.
    async fn next_messages(session: &mut LspSession, path: &Path) -> Vec<String> {
        let diagnostics: Vec<Diagnostic> =
            tokio::time::timeout(TIMEOUT, session.wait_for_accepted_diagnostics(path, 0))
                .await
                .unwrap();
        diagnostics
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect()
    }
}
