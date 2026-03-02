//
// table.rs
//
// Copyright (C) 2024-2026 by Posit Software, PBC
//
//

use harp::RObject;

#[derive(Clone)]
pub struct Table(RObject);

// Safety: `Table` is only accessed on the R thread (or in R idle tasks,
// which also run on the R thread).
unsafe impl Send for Table {}

impl Table {
    pub fn new(data: RObject) -> Self {
        Self(data)
    }

    pub fn get(&self) -> &RObject {
        &self.0
    }

    pub fn set(&mut self, data: RObject) {
        self.0 = data;
    }
}
