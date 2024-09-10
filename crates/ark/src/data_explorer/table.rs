//
// table.rs
//
// Copyright (C) 2024 by Posit Software, PBC
//
//

use std::sync::Arc;
use std::sync::Mutex;

use anyhow::anyhow;
use harp::RObject;

use crate::thread::RThreadSafe;

#[derive(Clone)]
pub struct Table {
    table: Arc<Mutex<Option<RThreadSafe<RObject>>>>,
}

impl Table {
    pub fn new(data: RThreadSafe<RObject>) -> Self {
        let table = Arc::new(Mutex::new(Some(data)));
        Self { table }
    }

    // Get can only be called from the main thread as it will also call
    // get in the RThreadSafe object.
    // Get only result in errors when the table is no longer available, so this
    // failing to get, can be used as a sign to cancel a task.
    pub fn get(&self) -> anyhow::Result<RObject> {
        let guard = self.table.lock().unwrap();
        let table = guard
            .as_ref()
            .ok_or(anyhow!("Table not found"))?
            .get()
            .clone();
        Ok(table)
    }

    pub fn set(&mut self, data: RThreadSafe<RObject>) {
        let mut table = self.table.lock().unwrap();
        *table = Some(data);
    }

    pub fn delete(&mut self) {
        let mut table = self.table.lock().unwrap();
        *table = None;
    }
}
