//! Inspect the user's R installation without using the kernel's R session.
//!
//! Scripts run in a separate R process. [`primitives`] caches its output in an
//! [`oak_cache::Cache`] keyed by the R build it describes. [`lazy_data`] reads
//! installed packages on every run.

pub mod lazy_data;
pub mod primitives;
