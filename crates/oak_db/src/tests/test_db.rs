use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;

use aether_url::UrlId;
use url::Url;

use crate::Db;
use crate::Root;
use crate::RootKind;
use crate::SourceGraph;
use crate::WorkspaceRoots;

pub(super) type Events = Arc<Mutex<Vec<salsa::Event>>>;

#[salsa::db]
#[derive(Clone)]
pub(super) struct TestDb {
    storage: salsa::Storage<Self>,
    events: Events,
    source_graph: Arc<OnceLock<SourceGraph>>,
    workspace_roots: Arc<OnceLock<WorkspaceRoots>>,
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
            source_graph: Arc::new(OnceLock::new()),
            workspace_roots: Arc::new(OnceLock::new()),
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
    fn source_graph(&self) -> SourceGraph {
        *self.source_graph.get_or_init(|| SourceGraph::empty(self))
    }

    fn workspace_roots(&self) -> WorkspaceRoots {
        *self
            .workspace_roots
            .get_or_init(|| WorkspaceRoots::empty(self))
    }
}

pub(super) fn file_url(name: &str) -> UrlId {
    UrlId::from_canonical(Url::parse(&format!("file:///{name}")).unwrap())
}

/// Build a fresh `RootKind::Workspace` `Root` at `path` with revision 0.
/// Each call allocates a new salsa entity; tests that need to assert
/// on root identity should retain the returned value.
pub(super) fn workspace_root(db: &TestDb, path: &str) -> Root {
    Root::new(db, file_url(path), RootKind::Workspace, 0)
}
