//
// library.rs
//
// Copyright (C) 2025 by Posit Software, PBC
//

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;

use super::package::Package;
use crate::lsp;

/// Lazily manages a list of known R packages by name
#[derive(Default, Clone, Debug)]
pub struct Library {
    /// Paths to library directories, i.e. what `base::libPaths()` returns.
    pub library_paths: Arc<Vec<PathBuf>>,

    packages: Arc<RwLock<HashMap<String, Option<Arc<Package>>>>>,
}

impl Library {
    pub fn new(library_paths: Vec<PathBuf>) -> Self {
        Self {
            packages: Arc::new(RwLock::new(HashMap::new())),
            library_paths: Arc::new(library_paths),
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
                lsp::log_error!("Can't load R package: {err:?}");
                None
            },
        };

        self.packages
            .write()
            .unwrap()
            .insert(name.to_string(), pkg.clone());

        pkg
    }

    /// Insert a package in the library for testing purposes.
    #[cfg(test)]
    pub fn insert(self, name: &str, package: Package) -> Self {
        self.packages
            .write()
            .unwrap()
            .insert(name.to_string(), Some(Arc::new(package)));
        self
    }

    fn load_package(&self, name: &str) -> anyhow::Result<Option<Package>> {
        for lib_path in self.library_paths.iter() {
            match Package::load_from_library(&lib_path, name)? {
                Some(pkg) => return Ok(Some(pkg)),
                None => (),
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
        let lib = Library::new(vec![temp_dir.path().to_path_buf()]);

        // First access loads from disk
        let pkg = lib.get(pkg_name).unwrap();
        assert_eq!(pkg.description.name, "mypkg");

        // Second access uses cache (note that we aren't testing that we are
        // indeed caching, just exercising the cache code path)
        assert!(lib.get(pkg_name).is_some());

        // Negative cache: missing package
        assert!(lib.get("notapkg").is_none());
        // Now cached as absent
        assert!(lib.get("notapkg").is_none());

        // Namespace is parsed
        assert_eq!(pkg.namespace.exports, vec!["bar", "foo"]);
        assert_eq!(pkg.namespace.imports, vec!["baz"]);
    }
}
