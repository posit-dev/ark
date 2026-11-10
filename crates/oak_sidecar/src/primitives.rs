//! Stubs for base R's primitive functions
//!
//! Primitives are implemented in C, so base's R sources don't define them. A sidecar R
//! process writes one stub per primitive into an R file that joins `base`'s files, so
//! name resolution and function detection treat primitives like other base functions.
//! [`PrimitivesCache`] caches that file per R build, under `primitives/v1/`.

use std::path::Path;
use std::path::PathBuf;

use oak_cache::set_readonly;
use oak_cache::short_hash;
use oak_cache::Cache;

const SCRIPT: &str = include_str!("../scripts/primitives.R");

const CACHE_VERSION: &str = "v1";

const STUBS_FILE_NAME: &str = "primitives.R";

/// Generates and caches primitive stubs for an R build.
#[derive(Debug)]
pub struct PrimitivesCache {
    r: PathBuf,
    cache: Cache,
}

impl PrimitivesCache {
    pub fn open(r: PathBuf) -> anyhow::Result<Self> {
        Ok(Self {
            r,
            cache: Cache::open(&Path::new("primitives").join(CACHE_VERSION))?,
        })
    }

    /// Open an isolated cache at `root`, without touching the shared cache.
    pub fn open_in(root: PathBuf, r: PathBuf) -> anyhow::Result<Self> {
        Ok(Self {
            r,
            cache: Cache::open_in(root)?,
        })
    }

    /// Return the cached stub directory for base's `Version:` and `Built:`
    /// fields, or `None` if absent.
    pub fn get(&self, version: &str, built: &str) -> Option<PathBuf> {
        self.cache.get(&key(version, built))
    }

    /// Generate stubs and return their cache directory, keyed by base's
    /// `Version:` and `Built:` fields.
    ///
    /// Return `None` if the executable's base version differs from `version`,
    /// or if generation or caching fails.
    pub fn insert(&self, version: &str, built: &str) -> Option<PathBuf> {
        self.cache
            .insert(&key(version, built), |dir| populate(&self.r, version, dir))
            .unwrap_or_else(|err| {
                log::error!("Failed to generate primitive stubs for R {version}: {err:?}");
                None
            })
    }
}

/// Cache key `{version}_{built_hash}`, from base's `DESCRIPTION`
///
/// R-devel builds can report the same version, but each build stamps its own `Built:`
/// field, so two builds get separate entries unless the 8-character hashes of their
/// `Built:` fields collide.
fn key(version: &str, built: &str) -> String {
    let built_hash = short_hash(built);
    format!("{version}_{built_hash}")
}

fn populate(r: &Path, version: &str, dir: &Path) -> anyhow::Result<bool> {
    let output = oak_r_process::run_text(r, SCRIPT, &[version], &[])?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!(
            "R script failed with {status} for R {version}: {stderr}",
            status = output.status
        ));
    }

    let stubs = String::from_utf8(output.stdout).map_err(|err| {
        anyhow::anyhow!("R script output was not valid UTF-8 for R {version}: {err}")
    })?;

    let path = dir.join(STUBS_FILE_NAME);
    std::fs::write(&path, stubs)?;
    set_readonly(&path)?;

    Ok(true)
}

#[cfg(test)]
mod tests {
    use crate::primitives::key;

    #[test]
    fn key_is_version_and_built_hash() {
        let key = key("4.5.2", "dummy");

        let (version, built_hash) = key.rsplit_once('_').unwrap();
        assert_eq!(version, "4.5.2");
        assert_eq!(built_hash.len(), 8);

        assert_eq!(crate::primitives::key("4.5.2", "dummy"), key);
        assert_ne!(crate::primitives::key("4.5.2", "dummy2"), key);
    }
}
