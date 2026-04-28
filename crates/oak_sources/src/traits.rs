use std::path::PathBuf;

/// Trait for the public API of any package cache
///
/// Implemented by the main [crate::PackageCache] itself, but also by
/// [crate::test::TestPackageCache] so that you can generate a test cache that doesn't
/// need internet access or access to a live R session.
pub trait PackageCache: std::fmt::Debug + Sync + Send {
    fn get(&self, package: &str) -> Option<PathBuf>;
}
