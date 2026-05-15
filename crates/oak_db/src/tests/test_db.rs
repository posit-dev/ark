use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;

use url::Url;

use crate::Db;
use crate::SourceGraph;

pub(super) type Events = Arc<Mutex<Vec<salsa::Event>>>;

#[salsa::db]
#[derive(Clone)]
pub(super) struct TestDb {
    storage: salsa::Storage<Self>,
    events: Events,
    source_graph: Arc<OnceLock<SourceGraph>>,
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
}

pub(super) fn file_url(name: &str) -> Url {
    Url::parse(&format!("file:///{name}")).unwrap()
}
