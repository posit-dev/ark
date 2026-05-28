use std::sync::Arc;
use std::sync::OnceLock;

use crate::Db;
use crate::DbInputs;
use crate::LibraryRoots;
use crate::OrphanRoot;
use crate::StaleRoot;
use crate::WorkspaceRoots;

/// Concrete Salsa database.
///
/// Holds singleton `WorkspaceRoots` / `LibraryRoots` / `OrphanRoot` /
/// `StaleRoot` inputs and lazy-initialises them on first access.
#[salsa::db]
#[derive(Clone, Default)]
pub struct OakDatabase {
    storage: salsa::Storage<Self>,
    workspace_roots: Arc<OnceLock<WorkspaceRoots>>,
    library_roots: Arc<OnceLock<LibraryRoots>>,
    orphan_root: Arc<OnceLock<OrphanRoot>>,
    stale_root: Arc<OnceLock<StaleRoot>>,
}

impl OakDatabase {
    pub fn new() -> Self {
        Self::default()
    }
}

#[salsa::db]
impl salsa::Database for OakDatabase {}

impl std::fmt::Debug for OakDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OakDatabase").finish_non_exhaustive()
    }
}

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

    fn stale_root(&self) -> StaleRoot {
        *self.stale_root.get_or_init(|| StaleRoot::empty(self))
    }
}

#[salsa::db]
impl Db for OakDatabase {
    fn file_by_url(&self, url: &aether_path::UrlId) -> Option<crate::File> {
        crate::db::file_by_url_query(self, url)
    }

    fn package_by_name(&self, name: &str) -> Option<crate::Package> {
        crate::db::package_by_name_query(self, name)
    }

    fn root_by_package(&self, pkg: crate::Package) -> Option<crate::Root> {
        crate::db::root_by_package_query(self, pkg)
    }

    fn live_roots(&self) -> &[crate::LiveRoot] {
        crate::db::live_roots_query(self)
    }
}
