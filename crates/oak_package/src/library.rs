use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;

use oak_sources::traits::PackageCache;
use stdext::result::ResultExt;

use crate::definitions::PackageDefinitions;
use crate::package::Package;

/// Lazily manages a list of known R packages by name
#[derive(Default, Clone, Debug)]
pub struct Library {
    /// Paths to library directories, i.e. what `base::libPaths()` returns.
    pub library_paths: Arc<Vec<PathBuf>>,

    /// Package cache for loading package sources
    ///
    /// Stored as `dyn PackageCache` so we can easily swap in a test cache during
    /// LSP feature testing
    package_cache: Option<Arc<dyn PackageCache>>,

    packages: Arc<RwLock<HashMap<String, Option<Arc<Package>>>>>,
}

impl Library {
    pub fn new(library_paths: Vec<PathBuf>, package_cache: Option<Arc<dyn PackageCache>>) -> Self {
        Self {
            library_paths: Arc::new(library_paths),
            package_cache,
            packages: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get a package by name, loading and caching it if necessary.
    /// Returns `None` if the package can't be found or loaded.
    pub fn get(&self, name: &str) -> Option<Arc<Package>> {
        // Try to get from cache first (could be `None` if we already tried to
        // load a non-existent or broken package)
        if let Some(entry) = self.packages.read().unwrap().get(name) {
            return entry.clone();
        }

        // Not cached, try to load
        let pkg = match self.load_package(name) {
            Ok(Some(pkg)) => Some(Arc::new(pkg)),
            Ok(None) => None,
            Err(err) => {
                log::error!("Can't load R package: {err:?}");
                None
            },
        };

        self.packages
            .write()
            .unwrap()
            .insert(name.to_string(), pkg.clone());

        pkg
    }

    /// Collect all top level definitions from a package's sources
    ///
    /// FIXME: This is currently very expensive as it reparses the package at every call.
    /// We expect this to be a tracked salsa function, which should memoize it
    /// efficiently.
    pub fn definitions(&self, name: &str) -> Option<PackageDefinitions> {
        let package_cache = self.package_cache.as_ref()?;
        let package = self.get(name)?;
        let directory = package_cache.get(&package.description().name)?;
        PackageDefinitions::load_from_directory(&directory, package.namespace()).log_err()
    }

    /// Insert a package in the library for testing purposes.
    #[cfg(any(test, feature = "testing"))]
    pub fn insert(self, name: &str, package: Package) -> Self {
        self.packages
            .write()
            .unwrap()
            .insert(name.to_string(), Some(Arc::new(package)));
        self
    }

    fn load_package(&self, name: &str) -> anyhow::Result<Option<Package>> {
        for lib_path in self.library_paths.iter() {
            if let Some(pkg) = Package::load_from_library(lib_path, name)? {
                return Ok(Some(pkg));
            }
        }

        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::fs::{self};
    use std::io::Write;

    use oak_package_metadata::namespace::Import;
    use tempfile::TempDir;

    use super::*;

    // Helper to create a temporary package directory with DESCRIPTION and NAMESPACE
    fn create_temp_package(
        pkg_name: &str,
        description: &str,
        namespace: &str,
    ) -> (TempDir, PathBuf) {
        let temp_dir = TempDir::new().unwrap();
        let pkg_dir = temp_dir.path().join(pkg_name);
        fs::create_dir(&pkg_dir).unwrap();

        let desc_path = pkg_dir.join("DESCRIPTION");
        let mut desc_file = File::create(&desc_path).unwrap();
        desc_file.write_all(description.as_bytes()).unwrap();

        let ns_path = pkg_dir.join("NAMESPACE");
        let mut ns_file = File::create(&ns_path).unwrap();
        ns_file.write_all(namespace.as_bytes()).unwrap();

        (temp_dir, pkg_dir)
    }

    #[test]
    fn test_load_and_cache_package() {
        let pkg_name = "mypkg";
        let description = r#"
Package: mypkg
Version: 1.0
        "#;
        let namespace = r#"
export(foo)
export(bar)
importFrom(pkg, baz)
        "#;

        let (temp_dir, _pkg_dir) = create_temp_package(pkg_name, description, namespace);

        // Library should point to the temp_dir as its only library path
        let lib = Library::new(vec![temp_dir.path().to_path_buf()], None);

        // First access loads from disk
        let pkg = lib.get(pkg_name).unwrap();
        assert_eq!(pkg.description().name, "mypkg");

        // Second access uses cache (note that we aren't testing that we are
        // indeed caching, just exercising the cache code path)
        assert!(lib.get(pkg_name).is_some());

        // Negative cache: missing package
        assert!(lib.get("notapkg").is_none());
        // Now cached as absent
        assert!(lib.get("notapkg").is_none());

        // Namespace is parsed
        assert_eq!(pkg.namespace().exports.to_vec(), vec!["bar", "foo"]);
        assert_eq!(pkg.namespace().imports, vec![Import {
            name: "baz".to_string(),
            package: "pkg".to_string()
        }]);
    }
}
