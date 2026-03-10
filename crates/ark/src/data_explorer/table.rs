//
// table.rs
//
// Copyright (C) 2024-2026 by Posit Software, PBC
//
//

use harp::RObject;

pub struct Table(RObject);

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
