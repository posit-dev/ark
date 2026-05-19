use std::sync::Arc;
use std::sync::OnceLock;

use crate::DbInputs;
use crate::LibraryRoots;
use crate::OrphanRoot;
use crate::WorkspaceRoots;

/// Concrete Salsa database.
///
/// Holds singleton `WorkspaceRoots` / `LibraryRoots` / `OrphanRoot`
/// inputs and lazy-initialises them on first access.
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
impl DbInputs for OakDatabase {
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
