mod base;
mod cache;
mod cran;
mod download;
mod fs;
mod hash;
mod installed_package;
mod srcref;
#[cfg(any(test, feature = "testing"))]
pub mod test;

use std::path::PathBuf;

pub use cache::new_cache_pair;
pub use cache::PackageCacheReader;
pub use cache::PackageCacheWriter;

/// Trait for an object that can retrieve package sources
///
/// Implemented by the [crate::cache::PackageCacheReader] itself, but also by
/// [crate::test::TestPackageCache] so that you can generate a test cache that doesn't
/// need internet access or access to a live R session.
pub trait PackageSources: std::fmt::Debug + Sync + Send {
    /// Returns a package to a directory with an `R/` subdirectory holding package sources
    fn get(&self, package: &str) -> Option<PathBuf>;
}
