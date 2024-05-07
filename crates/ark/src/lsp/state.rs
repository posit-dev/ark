use std::sync::Arc;

use dashmap::DashMap;
use parking_lot::Mutex;
use url::Url;

use crate::lsp::documents::Document;

#[derive(Clone, Debug)]
pub struct WorldState {
    pub documents: Arc<DashMap<Url, Document>>,
    pub workspace: Arc<Mutex<Workspace>>,
}

#[derive(Debug)]
pub struct Workspace {
    pub folders: Vec<Url>,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            folders: Default::default(),
        }
    }
}
