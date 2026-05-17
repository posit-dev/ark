use std::sync::Arc;
use std::sync::OnceLock;

use crate::Db;
use crate::LibraryRoots;
use crate::OrphanRoot;
use crate::WorkspaceRoots;

/// Concrete Salsa database. The canonical implementation of [`Db`].
///
/// Holds singleton `WorkspaceRoots` / `LibraryRoots` / `OrphanRoot`
/// inputs and lazy-initialises them on first access. All reads and
/// writes are inherited from the [`Db`] trait's methods (`file_by_url`,
/// `set_file`, `set_package`, etc.).
///
/// When future db-trait crates land (e.g. `oak_types::Db: oak_db::Db`),
/// they add their `impl` for `OakDatabase` externally via the orphan
/// rule (local trait, foreign type). No need for a separate aggregator
/// crate at the current scale.
///
/// `Clone` is cheap. Salsa shares the `Storage` across clones for
/// per-thread snapshots.
#[salsa::db]
#[derive(Clone, Default)]
pub struct OakDatabase {
    storage: salsa::Storage<Self>,
    workspace_roots: Arc<OnceLock<WorkspaceRoots>>,
    library_roots: Arc<OnceLock<LibraryRoots>>,
    orphan_root: Arc<OnceLock<OrphanRoot>>,
}

impl OakDatabase {
    pub fn new() -> Self {
        Self::default()
    }
}

#[salsa::db]
impl salsa::Database for OakDatabase {}

#[salsa::db]
impl Db for OakDatabase {
    fn workspace_roots(&self) -> WorkspaceRoots {
        *self
            .workspace_roots
            .get_or_init(|| WorkspaceRoots::empty(self))
    }

    fn library_roots(&self) -> LibraryRoots {
        *self.library_roots.get_or_init(|| LibraryRoots::empty(self))
    }

    fn orphan_root(&self) -> OrphanRoot {
        *self.orphan_root.get_or_init(|| OrphanRoot::empty(self))
    }
}
