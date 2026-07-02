//! Recover an installed R package's sources from `srcref` metadata
//!
//! [`SrcrefCache`] is a thin wrapper over an [`oak_cache::Cache`] rooted at
//! `srcref/v1/`. Each entry is a flat directory of `.R` files recovered from the
//! `srcref` attributes of an installed package, via a sidecar R session.

mod srcref;

use std::path::Path;
use std::path::PathBuf;

use oak_cache::Cache;
use sha2::Digest;
use sha2::Sha256;

/// Cache version
const CACHE_VERSION: &str = "v1";

/// LRU capacity for the cache
const CACHE_CAPACITY: usize = 200;

/// Recovers and caches an installed package's sources from `srcref` metadata
#[derive(Debug)]
pub struct SrcrefCache {
    /// Path to an R executable used to run the recovery script
    r: PathBuf,
    cache: Cache,
}

impl SrcrefCache {
    pub fn new(r: PathBuf) -> anyhow::Result<Self> {
        Ok(Self {
            r,
            cache: Cache::new(&format!("srcref/{CACHE_VERSION}"), CACHE_CAPACITY)?,
        })
    }

    /// Open a `SrcrefCache` rooted at an explicit directory instead of the shared cache
    ///
    /// Useful for integration tests that don't want to touch the real on disk cache.
    pub fn new_in(root: PathBuf, r: PathBuf) -> anyhow::Result<Self> {
        Ok(Self {
            r,
            cache: Cache::new_in(root, CACHE_CAPACITY)?,
        })
    }

    /// Get cached srcref sources if already present
    pub fn get(&self, name: &str, version: &str, built: &str) -> Option<PathBuf> {
        self.cache.get(&key(name, version, built))
    }

    /// Recover and cache srcref sources for an installed package
    ///
    /// Returns `None` if the package wasn't installed with srcrefs preserved, or recovery
    /// fails for any other reason.
    pub fn insert(
        &self,
        name: &str,
        version: &str,
        built: &str,
        library_path: &Path,
    ) -> Option<PathBuf> {
        self.cache
            .insert(&key(name, version, built), |dir| {
                srcref::populate(&self.r, name, version, library_path, dir)
            })
            .unwrap_or_else(|err| {
                log::error!("Failed to recover srcref sources for '{name}' {version}: {err:?}");
                None
            })
    }
}

/// Cache key `{name}_{version}_{built_hash}`
///
/// The `Built:` hash disambiguates reinstalls! A build timestamp changes on every
/// rebuild, so a rebuilt package gets a fresh key.
fn key(name: &str, version: &str, built: &str) -> String {
    let built_hash = hash(built);
    format!("{name}_{version}_{built_hash}")
}

/// First 8 hex characters of the SHA-256 of `contents`
fn hash(contents: &str) -> String {
    let mut hash = hex::encode(Sha256::digest(contents));
    hash.truncate(8);
    hash
}

#[cfg(test)]
mod tests {
    use crate::key;

    #[test]
    fn key_is_name_version_and_built_hash() {
        let key = key("generics", "0.1.4", "dummy");

        let (prefix, built_hash) = key.rsplit_once('_').unwrap();
        assert_eq!(prefix, "generics_0.1.4");
        assert_eq!(built_hash.len(), 8);

        // Same `Built:` is stable, a different `Built:` yields a different key
        assert_eq!(crate::key("generics", "0.1.4", "dummy"), key);
        assert_ne!(crate::key("generics", "0.1.4", "dummy2"), key);
    }
}
