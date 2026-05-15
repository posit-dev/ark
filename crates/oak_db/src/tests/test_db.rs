use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;

use aether_url::UrlId;
use url::Url;

use crate::Db;
use crate::LibraryRoots;
use crate::Root;
use crate::RootKind;
use crate::WorkspaceRoots;

pub(super) type Events = Arc<Mutex<Vec<salsa::Event>>>;

#[salsa::db]
#[derive(Clone)]
pub(super) struct TestDb {
    storage: salsa::Storage<Self>,
    events: Events,
    workspace_roots: Arc<OnceLock<WorkspaceRoots>>,
    library_roots: Arc<OnceLock<LibraryRoots>>,
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
impl Db for TestDb {
    fn workspace_roots(&self) -> WorkspaceRoots {
        *self
            .workspace_roots
            .get_or_init(|| WorkspaceRoots::empty(self))
    }

    fn library_roots(&self) -> LibraryRoots {
        *self
            .library_roots
            .get_or_init(|| LibraryRoots::empty(self))
    }
}

pub(super) fn file_url(name: &str) -> UrlId {
    UrlId::from_canonical(Url::parse(&format!("file:///{name}")).unwrap())
}

/// Build a fresh empty `RootKind::Workspace` `Root` at `path`. Each call
/// allocates a new salsa entity; tests that need to assert on root identity
/// should retain the returned value.
pub(super) fn workspace_root(db: &TestDb, path: &str) -> Root {
    Root::new(db, file_url(path), RootKind::Workspace, vec![], vec![])
}

/// Build a fresh empty `RootKind::Library` `Root` at `path`. Each call
/// allocates a new salsa entity.
pub(super) fn library_root(db: &TestDb, path: &str) -> Root {
    Root::new(db, file_url(path), RootKind::Library, vec![], vec![])
}

/// Register `scripts` under a fresh workspace root at the URL filesystem
/// root, and register that root in `WorkspaceRoots`. Used by tests that
/// need cross-file resolution via `source()` to find their scripts.
pub(super) fn register_scripts(db: &mut TestDb, scripts: Vec<crate::Script>) -> Root {
    use salsa::Setter;
    let root = workspace_root(db, "");
    root.set_scripts(db).to(scripts);
    db.workspace_roots().set_roots(db).to(vec![root]);
    root
}
