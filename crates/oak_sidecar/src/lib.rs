//! Inspect the user's R installation without using the kernel's R session.
//!
//! Scripts run in a separate R process. [`primitives`] caches its output in an
//! [`oak_cache::Cache`] keyed by the R build it describes. [`lazy_data`] reads
//! installed packages on every run.

use std::time::Duration;

pub mod lazy_data;
pub mod primitives;

/// Limit on each R run. The LSP runs sidecar batches one at a time on a single
/// thread, so a hung run would hold back every later batch and leave its
/// packages' answers incomplete. A timed-out run returns an error, which fails
/// only the requests that run serves: lazy data for every package that asked
/// for it, or `base`'s primitive stubs.
const R_TIMEOUT: Duration = Duration::from_secs(60);
