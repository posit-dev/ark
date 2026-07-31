//
// snapshot.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use oak_db::OakDatabase;

use super::catch_cancellation;
use crate::lsp::config::LspConfig;
use crate::lsp::db::ArkDb;
use crate::lsp::state::Workspace;
use crate::lsp::state::WorldState;

/// Read-only snapshot of [`WorldState`] handed to a background reader, so a
/// reader thread can't reach salsa input setters. Carries only the fields
/// readers actually use. Mirrors rust-analyzer's `GlobalStateSnapshot`.
#[derive(Debug)]
pub(crate) struct WorldStateSnapshot {
    /// Private so readers can only reach it through [`Self::db`].
    db: OakDatabase,
    pub(crate) workspace: Workspace,
    pub(crate) console_scopes: Vec<Vec<String>>,
    pub(crate) installed_packages: Vec<String>,
    pub(crate) config: LspConfig,
}

/// Minting lives here rather than in `state.rs` because
/// [`WorldStateSnapshot`]'s db field is private to this module.
impl WorldState {
    /// Take a read-only snapshot of the world for a background reader.
    ///
    /// The snapshot holds a Salsa handle, which parks the next main-loop write
    /// until it drops. That's safe on the [`AnalysisPool`], whose tasks unwind
    /// on cancellation. The only other caller is `handle_completion()`, which
    /// hands the snapshot to `r_task()` and blocks until it returns, so that
    /// handle can't outlive the tick that made it.
    pub(crate) fn snapshot(&self) -> WorldStateSnapshot {
        WorldStateSnapshot {
            db: self.db.snapshot(),
            console_scopes: self.console_scopes.clone(),
            installed_packages: self.installed_packages.clone(),
            config: self.config.clone(),
            workspace: self.workspace.clone(),
        }
    }
}

impl WorldStateSnapshot {
    /// Read-only access to the database. Returns `&dyn ArkDb` rather than
    /// `&OakDatabase` because `dyn ArkDb` is unsized, so a reader can't
    /// `.snapshot()` its way to an owned database and call setters on it.
    pub(crate) fn db(&self) -> &dyn ArkDb {
        &self.db
    }

    /// Whether salsa would unwind this handle's next query, because a writer is
    /// parked waiting for it to drop (or, in tests, because the token was armed
    /// by hand). `unwind_if_revision_cancelled()` reports by throwing, so we
    /// catch it to get an answer.
    pub(super) fn is_cancelled(&self) -> bool {
        catch_cancellation(|| salsa::Database::unwind_if_revision_cancelled(&self.db)).is_none()
    }

    /// The database's salsa cancellation token. Read-side only: it observes and
    /// arms cancellation, it doesn't mutate any input. Only cancellation tests
    /// arm it by hand.
    #[cfg(test)]
    pub(crate) fn cancellation_token(&self) -> salsa::CancellationToken {
        salsa::Database::cancellation_token(&self.db)
    }
}

#[cfg(test)]
mod tests {
    use crate::lsp::state::WorldState;

    /// A snapshot reports itself cancelled once salsa would unwind its next
    /// query, which is what the pool checks at dequeue.
    #[test]
    fn test_cancelled_snapshot_reports_cancelled() {
        let state = WorldState::default();

        let live = state.snapshot();
        assert!(!live.is_cancelled());

        let cancelled = state.snapshot();
        cancelled.cancellation_token().cancel();
        assert!(cancelled.is_cancelled());
    }
}
