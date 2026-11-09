//! Inspect the user's R installation without using the kernel's R session.
//!
//! Scripts run in a separate R process. [`primitives`] caches its output in an
//! [`oak_cache::Cache`] keyed by the R build it describes. [`lazy_data`] reads
//! installed packages on every run.

pub mod lazy_data;
pub mod primitives;

use std::path::Path;

use sha2::Digest;
use sha2::Sha256;

fn hash(contents: &str) -> String {
    let mut hash = hex::encode(Sha256::digest(contents));
    hash.truncate(8);
    hash
}

// Go-to-definition can open generated files. Read-only permissions discourage edits.
fn set_readonly(path: &Path) -> std::io::Result<()> {
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(path, permissions)
}
