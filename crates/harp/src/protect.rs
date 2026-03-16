//
// protect.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use libr::Rf_protect;
use libr::Rf_unprotect;
use libr::SEXP;

// NOTE: The RProtect struct uses R's stack-based object protection, and so is
// only appropriate for R objects with 'automatic' lifetime. In general, this
// should only be used when interfacing with native R APIs; general usages
// should use the RObject struct instead.
#[derive(Default)]
pub struct RProtect {
    count: i32,
}

impl RProtect {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, object: SEXP) -> SEXP {
        self.count += 1;
        unsafe { Rf_protect(object) }
    }
}

impl Drop for RProtect {
    /// SAFETY: Assumes that the R lock is held.
    fn drop(&mut self) {
        unsafe { Rf_unprotect(self.count) }
    }
}
