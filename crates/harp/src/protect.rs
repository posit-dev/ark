//
// protect.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use crate::r_api::Rf_protect;
use crate::r_api::Rf_unprotect;
use crate::r_api::SEXP;

// NOTE: The RProtect struct uses R's stack-based object protection, and so is
// only appropriate for R objects with 'automatic' lifetime. In general, this
// should only be used when interfacing with native R APIs; general usages
// should use the RObject struct instead.
pub struct RProtect {
    count: i32,
}

impl RProtect {
    pub fn new() -> Self {
        Self { count: 0 }
    }

    /// SAFETY: Requires that the R lock is held.
    pub fn add(&mut self, object: SEXP) -> SEXP {
        self.count += 1;
        return Rf_protect(object);
    }
}

impl Drop for RProtect {
    /// SAFETY: Requires that the R lock is held.
    fn drop(&mut self) {
        Rf_unprotect(self.count)
    }
}
