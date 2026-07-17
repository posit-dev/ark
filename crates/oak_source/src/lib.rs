//! Download and cache whole CRAN R package and base R package source trees
//!
//! [`SourceCache`] is a thin wrapper over [`oak_cache::Cache`] instances pointing into:
//!
//! - `source/v1/cran/{name}_{version}/`, a full unpacked CRAN package tarball
//! - `source/v1/r/{version}/`, the unpacked base R package sources for an R version
//! - `source/v1/compressed/{release}/`, the downloaded `r-source.tar.zst`, keyed by the
//!   `oak-r-sources` release and reused to populate any requested version's `r` entry
//!
//! The CRAN and base R caches keep the entire downloaded tree, unpacked eagerly with
//! files marked read only. Each method returns the cached folder. Navigation is left to
//! the caller, who knows the layout.

mod compressed;
mod cran;
mod download;
mod extract;
mod r;

use std::path::PathBuf;

use oak_cache::Cache;

/// Cache version
const CACHE_VERSION: &str = "v1";

/// Downloads and caches whole package source trees
///
/// Each cache holds its shared root lock for the life of this `SourceCache`, so any path
/// handed out stays valid as long as the `SourceCache` lives.
#[derive(Debug)]
pub struct SourceCache {
    cran: Cache,
    r: Cache,
    compressed: Cache,
}

impl SourceCache {
    pub fn open() -> anyhow::Result<Self> {
        Ok(Self {
            cran: Cache::open(&format!("source/{CACHE_VERSION}/cran"))?,
            r: Cache::open(&format!("source/{CACHE_VERSION}/r"))?,
            compressed: Cache::open(&format!("source/{CACHE_VERSION}/compressed"))?,
        })
    }

    /// Like [`SourceCache::open`], but rooted at an explicit `root` rather than the
    /// shared cache directory. Only useful for testing against a temp directory.
    pub fn open_in(root: PathBuf) -> anyhow::Result<Self> {
        Ok(Self {
            cran: Cache::open_in(root.join("cran"))?,
            r: Cache::open_in(root.join("r"))?,
            compressed: Cache::open_in(root.join("compressed"))?,
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

    /// Get cached base R source tree if already present
    pub fn get_r(&self, version: &str) -> Option<PathBuf> {
        let version = compressed::clamp(version);
        self.r.get(version)
    }

    /// Download and cache base R source tree
    ///
    /// Returns `None` if the archive is unavailable or holds no sources for `version`.
    pub fn insert_r(&self, version: &str) -> Option<PathBuf> {
        let version = compressed::clamp(version);
        self.r
            .insert(version, |dir| r::populate(version, dir, &self.compressed))
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

    /// Downloads the release archive, so requires internet access
    #[test]
    #[ignore]
    fn test_r_round_trip() {
        let dir = TempDir::new().unwrap();
        let source = SourceCache::open_in(dir.path().to_path_buf()).unwrap();

        // Miss before insert
        assert_eq!(source.get_r("4.5.0"), None);

        let root = source.insert_r("4.5.0").unwrap();

        // The `{version}/` prefix is stripped, so content lands under `{package}/R/`
        let help = root.join("utils").join("R").join("help.R");
        assert!(help.is_file());
        assert!(help.metadata().unwrap().permissions().readonly());

        // A second lookup is a cache hit returning the same dir
        assert_eq!(source.get_r("4.5.0"), Some(root));
    }

    /// Downloads the release archive to discover the version is missing, so requires
    /// internet access
    #[test]
    #[ignore]
    fn test_r_unknown_version() {
        let dir = TempDir::new().unwrap();
        let source = SourceCache::open_in(dir.path().to_path_buf()).unwrap();
        assert_eq!(source.insert_r("0.0.0"), None);
    }
}
