//
// analysis.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

//! Background analysis, the only place a salsa db handle lives off the main
//! loop.
//!
//! Two things maintain that invariant: [`WorldStateSnapshot`] is built only in
//! this module, and `OakDatabase` isn't `Clone`.

use std::panic::AssertUnwindSafe;

mod pool;
mod refresh;
mod snapshot;
mod warmup;

pub(crate) use pool::AnalysisPool;
pub(crate) use refresh::DiagnosticsReady;
pub(crate) use refresh::DiagnosticsState;
pub(crate) use snapshot::WorldStateSnapshot;
pub(crate) use warmup::warm_workspace_index;

/// Run `f`, swallowing a salsa cancellation as `None`. Any other panic propagates.
fn catch_cancellation<T>(f: impl FnOnce() -> T) -> Option<T> {
    salsa::Cancelled::catch(AssertUnwindSafe(f)).ok()
}
