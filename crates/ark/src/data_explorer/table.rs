//
// table.rs
//
// Copyright (C) 2024 by Posit Software, PBC
//
//

use anyhow::anyhow;
use dashmap::DashMap;
use harp::RObject;
use once_cell::sync::Lazy;

use crate::thread::RThreadSafe;

// Stores the table R objects objects that contain the data for each
// data explorer instance.
// This allows for background threads to easilly access the
// instance related data without having to rely on the lifetime
// of data explorer execution thread.
// Since this is a DashMap, it's safe to access it's underlying data from
// background threads, without the need for synchronization.
static DATA_EXPLORER_TABLES: Lazy<DashMap<String, RThreadSafe<RObject>>> =
    Lazy::new(|| DashMap::new());

// Abstracts away details on accessing the data explorer instance table.
// It's trivially copyable and cloneable since it's just a string, so
// it can be easily moved to background threads.
// Call `get()` to obtain the RObject for the table and `set` to modify
// the current value.
// Note: When a Table instance is deleted, nothing happens to the global store
// of tables, thus one must explictly call `Table.delete` before loosing the refence for it.
// In our case, we guarantee that the table is deleted by implementing `Drop` for the
// Data Explorer instance.
#[derive(Clone)]
pub struct Table {
    comm_id: String,
}

impl Table {
    pub fn new(comm_id: String, data: RObject) -> Self {
        let table = Self { comm_id };
        table.set(data);
        table
    }
    // `get` can only be called from the R main thread and will panick
    // otherwise.
    pub fn get(&self) -> anyhow::Result<RObject> {
        DATA_EXPLORER_TABLES
            .get(&self.comm_id)
            .and_then(|x| Some(x.get().clone()))
            .ok_or(anyhow!("Data explorer table has been deleted"))
    }
    pub fn set(&self, data: RObject) {
        DATA_EXPLORER_TABLES.insert(self.comm_id.clone(), RThreadSafe::new(data));
    }
    pub fn delete(&self) {
        if let None = DATA_EXPLORER_TABLES.remove(&self.comm_id) {
            log::warn!("The table no longer exists");
        }
    }
}
