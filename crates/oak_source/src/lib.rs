//! Download and cache whole CRAN package source trees
//!
//! [`SourceCache`] is a thin wrapper over an [`oak_cache::Cache`] pointing into
//! `source/v1/cran/{name}_{version}/`, a full unpacked CRAN package tarball. It keeps the
//! entire downloaded tree, unpacked eagerly with files marked read only. Each method
//! returns the cached folder. Navigation is left to the caller, who knows the layout.

mod cran;
mod download;
mod extract;

use std::path::PathBuf;

use oak_cache::Cache;

/// Cache version
const CACHE_VERSION: &str = "v1";

/// Downloads and caches whole package source trees
///
/// The cache holds its shared root lock for the life of this `SourceCache`, so any path
/// handed out stays valid as long as the `SourceCache` lives.
#[derive(Debug)]
pub struct SourceCache {
    cran: Cache,
}

impl SourceCache {
    pub fn open() -> anyhow::Result<Self> {
        Ok(Self {
            cran: Cache::open(&format!("source/{CACHE_VERSION}/cran"))?,
        })
    }

    /// Like [`SourceCache::open`], but rooted at an explicit `root` rather than the
    /// shared cache directory. Only useful for testing against a temp directory.
    pub fn open_in(root: PathBuf) -> anyhow::Result<Self> {
        Ok(Self {
            cran: Cache::open_in(root.join("cran"))?,
        })
    }

    /// Get cached CRAN package source tree if already present
    pub fn get_cran(&self, name: &str, version: &str) -> Option<PathBuf> {
        self.cran.get(&format!("{name}_{version}"))
    }

    /// Download and cache CRAN package source tree
    ///
    /// Returns `None` if the package isn't on CRAN or the download fails.
    pub fn insert_cran(&self, name: &str, version: &str) -> Option<PathBuf> {
        self.cran
            .insert(&format!("{name}_{version}"), |dir| {
                cran::populate(name, version, dir)
            })
            .unwrap_or_else(|err| {
                log::error!("Failed to download '{name}' {version} from CRAN: {err:?}");
                None
            })
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use crate::SourceCache;

    /// Requires internet access
    #[test]
    fn test_cran_round_trip() {
        let dir = TempDir::new().unwrap();
        let source = SourceCache::open_in(dir.path().to_path_buf()).unwrap();

        // Miss before insert
        assert_eq!(source.get_cran("vctrs", "0.7.2"), None);

        let root = source.insert_cran("vctrs", "0.7.2").unwrap();

        // The top-level `vctrs/` is stripped, so content lands directly under `root`
        assert!(root.join("R").is_dir());
        assert!(root.join("DESCRIPTION").is_file());

        // Unpacked files are read only
        for entry in std::fs::read_dir(root.join("R")).unwrap() {
            let metadata = entry.unwrap().metadata().unwrap();
            assert!(metadata.permissions().readonly());
        }

        // A second lookup is a cache hit returning the same dir
        assert_eq!(source.get_cran("vctrs", "0.7.2"), Some(root));
    }

    /// Requires internet access
    #[test]
    fn test_cran_not_found() {
        let dir = TempDir::new().unwrap();
        let source = SourceCache::open_in(dir.path().to_path_buf()).unwrap();
        assert_eq!(
            source.insert_cran("definitely_not_a_package", "0.0.0"),
            None
        );
    }
}
