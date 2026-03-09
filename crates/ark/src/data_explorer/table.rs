//
// table.rs
//
// Copyright (C) 2024-2026 by Posit Software, PBC
//
//

use harp::RObject;

use crate::thread::RThreadSafe;

pub struct Table(RThreadSafe<RObject>);

impl Table {
    pub fn new(data: RObject) -> Self {
        Self(RThreadSafe::new(data))
    }

    pub fn get(&self) -> &RObject {
        self.0.get()
    }

    pub fn set(&mut self, data: RObject) {
        self.0 = RThreadSafe::new(data);
    }

    /// Clone the table for use in an idle task. Must be called on the R thread.
    pub fn clone_for_task(&self) -> Self {
        Self(RThreadSafe::new(self.0.get().clone()))
    }
}
