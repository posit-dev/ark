//
// table.rs
//
// Copyright (C) 2024-2026 by Posit Software, PBC
//
//

use std::sync::Arc;
use std::sync::Mutex;

use anyhow::anyhow;
use harp::RObject;

#[derive(Clone)]
pub struct Table {
    table: Arc<Mutex<Option<RObject>>>,
}

// Safety: `Table` is only accessed from the R thread or from R idle tasks
// (which also run on the R thread).
unsafe impl Send for Table {}

impl Table {
    pub fn new(data: RObject) -> Self {
        let table = Arc::new(Mutex::new(Some(data)));
        Self { table }
    }

    // Fails when the table has been deleted, which can be used as a signal
    // to cancel a task.
    pub fn get(&self) -> anyhow::Result<RObject> {
        let guard = self.table.lock().unwrap();
        let table = guard.as_ref().ok_or(anyhow!("Table not found"))?.clone();
        Ok(table)
    }

    pub fn set(&mut self, data: RObject) {
        let mut table = self.table.lock().unwrap();
        *table = Some(data);
    }

    pub fn delete(&mut self) {
        let mut table = self.table.lock().unwrap();
        *table = None;
    }
}
