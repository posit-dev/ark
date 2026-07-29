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
#[derive(Default)]
pub struct OakDatabase {
    storage: salsa::Storage<Self>,
    workspace_roots: Arc<OnceLock<WorkspaceRoots>>,
    library_roots: Arc<OnceLock<LibraryRoots>>,
    orphan_root: Arc<OnceLock<OrphanRoot>>,
    stale_root: Arc<OnceLock<StaleRoot>>,
    // Clone counter that represents how many background readers have cloned the database
    holds: Arc<()>,
}

impl OakDatabase {
    pub fn new() -> Self {
        Self::default()
    }

    /// A snapshot handle onto the database for a background reader.
    ///
    /// When the main loop needs to write to a `&mut OakDatabase`, it gets
    /// parked by Salsa until all snapshot handles have been dropped. Only
    /// create a snapshot for cancellable CPU-bound tasks that either query
    /// Salsa or periodically check for cancellation.
    ///
    /// Keep `OakDatabase` non-`Clone`, this should be the only way to create a
    /// Salsa handle.
    pub fn snapshot(&self) -> Self {
        Self {
            storage: self.storage.clone(),
            workspace_roots: Arc::clone(&self.workspace_roots),
            library_roots: Arc::clone(&self.library_roots),
            orphan_root: Arc::clone(&self.orphan_root),
            stale_root: Arc::clone(&self.stale_root),
            holds: Arc::clone(&self.holds),
        }
    }

    // Number of live clones of this db (always >= 1, the caller itself). A
    // write through `&mut db` parks until this reaches 1, so a value > 1 here
    // means a write right now would block on that many outstanding handles.
    pub fn outstanding_holds(&self) -> usize {
        Arc::strong_count(&self.holds)
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
    fn file_by_path(&self, path: &aether_path::FilePath) -> Option<crate::File> {
        crate::db::file_by_path_query(self, path)
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
