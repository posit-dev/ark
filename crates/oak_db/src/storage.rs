use std::io;
use std::sync::Arc;
use std::sync::OnceLock;

use camino::Utf8Path;

use crate::file_reader::DiskFileReader;
use crate::file_reader::FileReader;
use crate::Db;
use crate::DbInputs;
use crate::EditorBuffers;
use crate::IdentityMap;
use crate::LibraryRoots;
use crate::OpenNotebooks;
use crate::OrphanRoot;
use crate::SourceDb;
use crate::WorkspaceRoots;

/// Concrete Salsa database.
///
/// Singleton inputs are initialised on first access. Both these inputs and
/// the [`IdentityMap`] are shared across database snapshots.
#[salsa::db]
pub struct OakDatabase {
    storage: salsa::Storage<Self>,
    file_reader: Arc<dyn FileReader>,
    workspace_roots: Arc<OnceLock<WorkspaceRoots>>,
    library_roots: Arc<OnceLock<LibraryRoots>>,
    orphan_root: Arc<OnceLock<OrphanRoot>>,
    identity_map: Arc<IdentityMap>,
    open_notebooks: Arc<OnceLock<OpenNotebooks>>,
    editor_buffers: EditorBuffers,
    // Clone counter that represents how many background readers have cloned the database
    holds: Arc<()>,
}

impl OakDatabase {
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a database whose queries read from the supplied reader.
    pub(crate) fn with_file_reader(reader: impl FileReader + 'static) -> Self {
        Self {
            storage: salsa::Storage::default(),
            file_reader: Arc::new(reader),
            workspace_roots: Arc::default(),
            library_roots: Arc::default(),
            orphan_root: Arc::default(),
            identity_map: Arc::default(),
            open_notebooks: Arc::default(),
            editor_buffers: EditorBuffers::default(),
            holds: Arc::default(),
        }
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
            file_reader: Arc::clone(&self.file_reader),
            workspace_roots: Arc::clone(&self.workspace_roots),
            library_roots: Arc::clone(&self.library_roots),
            orphan_root: Arc::clone(&self.orphan_root),
            identity_map: Arc::clone(&self.identity_map),
            open_notebooks: Arc::clone(&self.open_notebooks),
            // Snapshots never write, so they don't need the open set.
            editor_buffers: EditorBuffers::default(),
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

impl Default for OakDatabase {
    fn default() -> Self {
        Self::with_file_reader(DiskFileReader)
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
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        self.file_reader.read_to_string(path)
    }

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

    fn identity_map(&self) -> &IdentityMap {
        &self.identity_map
    }

    fn open_notebooks(&self) -> OpenNotebooks {
        *self
            .open_notebooks
            .get_or_init(|| OpenNotebooks::empty(self))
    }

    fn editor_buffers_mut(&mut self) -> &mut EditorBuffers {
        &mut self.editor_buffers
    }
}

#[salsa::db]
impl SourceDb for OakDatabase {
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

#[salsa::db]
impl Db for OakDatabase {}
