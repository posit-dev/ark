//! Download and cache whole R package and R version source trees
//!
//! [`SourceCache`] is a thin wrapper over two [`oak_cache::Cache`] instances pointing
//! into:
//!
//! - `source/v1/cran/{name}_{version}/`, a full unpacked CRAN package tarball
//! - `source/v1/r/{version}/`, a full unpacked R version source tarball
//!
//! Both keep the entire downloaded tree, unpacked eagerly with files marked read only.
//! Each method returns the cached folder. Navigation is left to the caller, who knows the
//! layout.

mod cran;
mod download;
mod extract;
mod r;

use std::path::PathBuf;

use oak_cache::Cache;

/// Cache version
const CACHE_VERSION: &str = "v1";

/// LRU capacity for cached CRAN package source trees
const CRAN_CAPACITY: usize = 200;

/// LRU capacity for cached R version source trees, kept small because each is large
const R_CAPACITY: usize = 5;

/// Downloads and caches whole package / R version source trees
///
/// Each cache holds its shared root lock for the life of this `SourceCache`, so any path
/// handed out stays valid as long as the `SourceCache` lives.
#[derive(Debug)]
pub struct SourceCache {
    cran: Cache,
    r: Cache,
}

impl SourceCache {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            cran: Cache::new(&format!("source/{CACHE_VERSION}/cran"), CRAN_CAPACITY)?,
            r: Cache::new(&format!("source/{CACHE_VERSION}/r"), R_CAPACITY)?,
        })
    }

    /// Like [`SourceCache::new`], but rooted at an explicit `root` rather than the shared
    /// cache directory. Only useful for testing against a temp directory.
    pub fn new_in(root: PathBuf) -> anyhow::Result<Self> {
        Ok(Self {
            cran: Cache::new_in(root.join("cran"), CRAN_CAPACITY)?,
            r: Cache::new_in(root.join("r"), R_CAPACITY)?,
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

    /// Get cached R source tree if already present
    pub fn get_r(&self, version: &str) -> Option<PathBuf> {
        self.r.get(version)
    }

    /// Download and cache R source tree
    ///
    /// Returns `None` if the tarball isn't on CRAN or the download fails.
    pub fn insert_r(&self, version: &str) -> Option<PathBuf> {
        self.r
            .insert(version, |dir| r::populate(version, dir))
            .unwrap_or_else(|err| {
                log::error!("Failed to download R {version} source: {err:?}");
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
        let source = SourceCache::new_in(dir.path().to_path_buf()).unwrap();

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
        let source = SourceCache::new_in(dir.path().to_path_buf()).unwrap();
        assert_eq!(
            source.insert_cran("definitely_not_a_package", "0.0.0"),
            None
        );
    }

    /// Requires internet access and downloads a large tarball of the R sources
    #[ignore = "Downloads a large R source tarball"]
    #[test]
    fn test_r_round_trip() {
        let dir = TempDir::new().unwrap();
        let source = SourceCache::new_in(dir.path().to_path_buf()).unwrap();

        let root = source.insert_r("4.5.0").unwrap();

        // The top-level `R-4.5.0/` is stripped. Spot check: `utils` has a well-known
        // `help.R` file inside the unpacked tree.
        let help = root.join("src/library/utils/R/help.R");
        assert!(help.is_file());
        assert!(help.metadata().unwrap().permissions().readonly());

        assert_eq!(source.get_r("4.5.0"), Some(root));
    }

    /// Requires internet access
    #[test]
    fn test_r_unknown_version() {
        let dir = TempDir::new().unwrap();
        let source = SourceCache::new_in(dir.path().to_path_buf()).unwrap();
        assert_eq!(source.insert_r("0.0.0"), None);
    }
}
