//! Concrete `Db` for query-level unit tests.
//!
//! Supplies singleton inputs, immutable in-memory file contents, and a Salsa
//! event recorder for query execution counts. Missing fixture files never fall
//! through to the host filesystem.

use std::io;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;

use aether_path::FilePath;
use camino::Utf8Component;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use oak_package_metadata::namespace::Namespace;
use rustc_hash::FxHashMap;
use salsa::plumbing::AsId;
use salsa::Setter;

use crate::Db;
use crate::DbInputs;
use crate::File;
use crate::FileRevision;
use crate::LibraryRoots;
use crate::OrphanRoot;
use crate::Package;
use crate::Root;
use crate::RootKind;
use crate::SourceDb;
use crate::StaleRoot;
use crate::WorkspaceRoots;

type Events = Arc<Mutex<Vec<salsa::Event>>>;

#[salsa::db]
#[derive(Clone)]
pub(super) struct TestDb {
    storage: salsa::Storage<Self>,
    events: Events,
    files: FxHashMap<Utf8PathBuf, String>,
    workspace_roots: Arc<OnceLock<WorkspaceRoots>>,
    library_roots: Arc<OnceLock<LibraryRoots>>,
    orphan_root: Arc<OnceLock<OrphanRoot>>,
    stale_root: Arc<OnceLock<StaleRoot>>,
}

impl TestDb {
    pub(super) fn new() -> Self {
        let events = Events::default();
        let storage = salsa::Storage::new(Some(Box::new({
            let events = events.clone();
            move |event| {
                events.lock().unwrap().push(event);
            }
        })));
        Self {
            storage,
            events,
            files: FxHashMap::default(),
            workspace_roots: Arc::new(OnceLock::new()),
            library_roots: Arc::new(OnceLock::new()),
            orphan_root: Arc::new(OnceLock::new()),
            stale_root: Arc::new(OnceLock::new()),
        }
    }

    /// Supply immutable file contents before any queries run.
    pub(super) fn with_files(files: impl IntoIterator<Item = (Utf8PathBuf, String)>) -> Self {
        Self {
            files: files.into_iter().collect(),
            ..Self::new()
        }
    }

    /// Matches query names by substring in the key's `Debug` representation.
    pub(super) fn executions(&self, name: &str) -> usize {
        self.count_executions(|key| format!("{key:?}").contains(name))
    }

    /// Like [`TestDb::executions()`], restricted to `file`.
    ///
    /// Only valid when [`File`] is the query's sole key, so Salsa uses its ID
    /// directly. Queries such as [`File::cross_file_layers()`] intern a tuple
    /// of arguments instead, so comparing their key index to a file ID cannot
    /// identify that file's executions.
    pub(super) fn executions_for(&self, name: &str, file: File) -> usize {
        let id = file.as_id();
        self.count_executions(|key| key.key_index() == id && format!("{key:?}").contains(name))
    }

    /// [`salsa::attach()`] lets the key's `Debug` formatter resolve query names
    /// using this database.
    fn count_executions(&self, matches: impl Fn(salsa::DatabaseKeyIndex) -> bool) -> usize {
        salsa::attach(self, || {
            self.events
                .lock()
                .unwrap()
                .iter()
                .filter(|event| match &event.kind {
                    salsa::EventKind::WillExecute { database_key } => matches(*database_key),
                    _ => false,
                })
                .count()
        })
    }
}

#[salsa::db]
impl salsa::Database for TestDb {}

#[salsa::db]
impl DbInputs for TestDb {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| io::ErrorKind::NotFound.into())
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

    fn stale_root(&self) -> StaleRoot {
        *self.stale_root.get_or_init(|| StaleRoot::empty(self))
    }
}

#[salsa::db]
impl SourceDb for TestDb {
    fn file_by_path(&self, path: &FilePath) -> Option<crate::File> {
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

pub(super) fn file_path(name: &str) -> FilePath {
    let root = if cfg!(windows) { "C:/" } else { "/" };
    let path = Utf8Path::new(root).join(name.trim_start_matches('/'));
    match FilePath::from_path_buf(path.into_std_path_buf()) {
        Some(path) => path,
        None => panic!("Invalid fixture path: {name}"),
    }
}

/// Render fixture names without a platform-specific root or URL escaping.
pub(crate) fn path_name(path: &FilePath) -> String {
    let Some(path) = path.as_path() else {
        panic!("Expected a filesystem fixture path: {path}");
    };
    path.components()
        .filter_map(|component| match component {
            Utf8Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Build a fresh empty `RootKind::Workspace` `Root` at `path`. Each
/// call allocates a new salsa entity; tests that need to assert on
/// root identity should retain the returned value.
pub(super) fn workspace_root(db: &impl Db, path: &str) -> Root {
    Root::new(db, file_path(path), RootKind::Workspace, vec![], vec![])
}

/// Build a fresh empty `RootKind::Library` `Root` at `path`.
pub(super) fn library_root(db: &impl Db, path: &str) -> Root {
    Root::new(db, file_path(path), RootKind::Library, vec![], vec![])
}

/// Build a package `pkg_name` with the given namespace and `R/` files (each
/// back-pointing to it), rooted under `ws/{pkg_name}`. Not registered on any
/// root, so callers can assemble several packages into one workspace. Returns
/// the package and its files in the given order.
pub(super) fn make_package(
    db: &mut TestDb,
    pkg_name: &str,
    namespace: Namespace,
    files: &[(&str, &str)],
) -> (Package, Vec<File>) {
    let pkg = Package::new(
        db,
        file_path(&format!("ws/{pkg_name}/DESCRIPTION")),
        pkg_name.to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        Some(namespace),
        Vec::new(),
        Vec::new(),
    );
    let entities: Vec<File> = files
        .iter()
        .map(|(path, contents)| {
            File::new(
                db,
                file_path(path),
                FileRevision::zero(),
                Some(contents.to_string()),
                Some(pkg),
            )
        })
        .collect();
    pkg.set_files(db).to(entities.clone());
    (pkg, entities)
}

#[salsa::db]
impl Db for TestDb {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_names_are_path_data() {
        for name in ["ws/a #?%.R", "abs/b.R"] {
            assert_eq!(path_name(&file_path(name)), name);
            assert_eq!(file_path(&format!("/{name}")), file_path(name));
        }
    }
}
