//! Scanners and update helpers for [`oak_db`].
//!
//! `oak_db` is read-only: queries that walk the input graph. `oak_scan`
//! is the write side: it walks the filesystem, parses package metadata,
//! and pushes the result into salsa inputs through a small set of named
//! helpers.
//!
//! [`oak_db::IdentityMap`] preserves `File` identity by path and `Package`
//! identity by `DESCRIPTION` path, independently of container membership.
//! Roots are reused by path while they remain live. Rescans update existing
//! entities in place, preserving downstream Salsa caches for files whose
//! content is unchanged, including files that disappear and reappear during
//! git branch switching. Reusing input entities also limits cache growth
//! because Salsa does not garbage-collect them.
//!
//! The trade-off is a small placement invariant: `file.package` must
//! agree with which container Vec holds the file (`pkg.files`,
//! `root.scripts`, or `orphan_root().files`). The helpers in this
//! crate are the only intended callers of the placement-affecting
//! setters on `oak_db`'s input structs.

mod environments;
mod eviction;
mod inputs;
mod library;
mod packages;
mod scheduler;
mod watch;

#[cfg(test)]
mod tests;

pub use environments::list_libraries;
pub use environments::LibraryListing;
pub use inputs::DbScan;
pub use inputs::FileEntry;
pub use library::InstalledPackageEntry;
pub use packages::is_environment_sentinel;
pub use scheduler::ScanCompleted;
pub use scheduler::ScanRequest;
pub use scheduler::ScanScheduler;
