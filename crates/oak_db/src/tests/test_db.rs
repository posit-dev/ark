//! Minimal concrete `Db` for query-level unit tests.
//!
//! Lives here so `file.rs` tests can exercise `File::parse` /
//! `File::semantic_index` without depending on `oak_storage`. Provides
//! the three input accessors (lazy-init `OnceLock`) and a salsa-event
//! recorder so tests can assert on query execution counts.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;

use aether_url::UrlId;
use url::Url;

use crate::Db;
use crate::DbInputs;
use crate::LibraryRoots;
use crate::OrphanRoot;
use crate::Root;
use crate::RootKind;
use crate::StaleRoot;
use crate::WorkspaceRoots;

type Events = Arc<Mutex<Vec<salsa::Event>>>;

#[salsa::db]
#[derive(Clone)]
pub(super) struct TestDb {
    storage: salsa::Storage<Self>,
    events: Events,
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
            workspace_roots: Arc::new(OnceLock::new()),
            library_roots: Arc::new(OnceLock::new()),
            orphan_root: Arc::new(OnceLock::new()),
            stale_root: Arc::new(OnceLock::new()),
        }
    }

    /// Count `WillExecute` events whose `database_key`'s Debug form
    /// contains `name`. Salsa's `DatabaseKeyIndex::fmt` resolves the
    /// underlying function name only when a database is attached to the
    /// current thread, so we wrap the scan in `salsa::attach`.
    pub(super) fn executions(&self, name: &str) -> usize {
        salsa::attach(self, || {
            self.events
                .lock()
                .unwrap()
                .iter()
                .filter(|event| match &event.kind {
                    salsa::EventKind::WillExecute { database_key } => {
                        format!("{database_key:?}").contains(name)
                    },
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
impl Db for TestDb {
    fn file_by_url(&self, url: &UrlId) -> Option<crate::File> {
        crate::db::file_by_url_query(self, url)
    }

    fn package_by_name(&self, name: &str) -> Option<crate::Package> {
        crate::db::package_by_name_query(self, name)
    }

    fn package_by_url(&self, url: &UrlId) -> Option<crate::Package> {
        crate::db::package_by_url_query(self, url)
    }

    fn root_by_package(&self, pkg: crate::Package) -> Option<crate::Root> {
        crate::db::root_by_package_query(self, pkg)
    }

    fn live_roots(&self) -> &[crate::LiveRoot] {
        crate::db::live_roots_query(self)
    }
}

pub(super) fn file_url(name: &str) -> UrlId {
    // `Url::to_file_path` on Windows requires a drive-letter prefix, so
    // synthesize one for tests. Linux is happy with rootless paths.
    let url = if cfg!(windows) {
        Url::parse(&format!("file:///C:/{name}")).unwrap()
    } else {
        Url::parse(&format!("file:///{name}")).unwrap()
    };
    UrlId::from_canonical(url)
}

/// Build a fresh empty `RootKind::Workspace` `Root` at `path`. Each
/// call allocates a new salsa entity; tests that need to assert on
/// root identity should retain the returned value.
pub(super) fn workspace_root(db: &impl Db, path: &str) -> Root {
    Root::new(db, file_url(path), RootKind::Workspace, vec![], vec![])
}

/// Build a fresh empty `RootKind::Library` `Root` at `path`.
pub(super) fn library_root(db: &impl Db, path: &str) -> Root {
    Root::new(db, file_url(path), RootKind::Library, vec![], vec![])
}
