mod cran;
mod fs;
mod srcref;

use std::collections::HashMap;
use std::fs::read_to_string;
use std::path::Path;
use std::path::PathBuf;

use chrono::DateTime;
use chrono::TimeDelta;
use chrono::Utc;
use oak_fs::file_lock;
use oak_fs::file_lock::FileLock;
use oak_package::package_description::Description;
use oak_package::package_description::Repository;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;

/// Name of the root lock file and the per-key lock file.
const LOCK_FILENAME: &str = ".lock";

/// Name of the completion sentinel written last in each cache entry.
const METADATA_FILENAME: &str = ".metadata";

/// Minimum age before `clean()` will evict a stale entry.
///
/// Gives users a window to revert CRAN <-> dev swaps (or similar) without losing the
/// cached CRAN build they had before the swap (dev builds will always be dropped due to
/// the DESCRIPTION `Build:` timestamp being different).
const ONE_WEEK: TimeDelta = TimeDelta::weeks(1);

/// A two-level cache (on disk + in memory) of extracted R package sources.
///
/// # On disk layout
///
/// The on disk cache at `<cache_dir>/oak/sources/v1/` is the source of truth
/// across all sessions. Each entry is a flat folder:
///
/// ```text
/// <cache_dir>/oak/sources/v1/
///     .lock
///     {package}_{version}_libpath-{hash}_description-{hash}/
///         .lock
///         .metadata
///         DESCRIPTION
///         NAMESPACE
///         R/
/// ```
///
/// Files other than `.lock` and `.metadata` are marked read only. `.metadata`
/// is written last and is the sole completion sentinel. An entry without it
/// is considered garbage and will be wiped by the next writer for that key.
///
/// The combination of `libpath-{hash}` and `description-{hash}` are enough to make a
/// unique key. The same R package could be installed in multiple libraries (even for the
/// same R version), so libpath matters and is also recorded in `.metadata` as one of the
/// signals that allows us to clean up a stale source folder. And the DESCRIPTION hash is
/// unique due to the `Built` field, which includes a timestamp of when the package was
/// built (either by CRAN or the user).
///
/// # Locking
///
/// The cache root `.lock` can be locked as shared or exclusive:
///
/// - **Shared root lock** is held for the lifetime of this `PackageSourcesCache`. The
///   invariant is that if you hold a shared root lock, you can read from or append new
///   entries to the cache, but never delete from it. Multiple sessions can hold this
///   simultaneously. The main purpose is to prevent cleanup from running so that any
///   PathBuf handed out by [`get`] stays valid while this lock is held.
///
/// - **Exclusive root lock** is only attempted to be taken at startup to run [`clean`].
///   Skipped if any other session already holds a shared root lock, which is fine, we
///   will just attempt to clean next time.
///
/// There are also per-key exclusive locks at
/// `{package}_{version}_libpath-{hash}_description-{hash}/.lock`. When appending a new
/// entry to the cache, a per-key exclusive lock must be taken first to ensure that two
/// sessions don't write to the same directory simultaneously.
///
/// This setup allows multiple sessions (or even one session) to add to the cache in
/// parallel, while guaranteeing that they cannot interfere with each other or a cleanup
/// run.
///
/// # In memory cache
///
/// [`get`] resolves the key and caches the resulting `Sources(PathBuf)` or `NoSources` in
/// memory. This is mostly to avoid re-attempting to generate sources for packages that we
/// know already have `NoSources`. Results for packages that weren't installed at all are
/// not cached, so later installs within the same session can still attempt a source
/// generation.
///
/// # Cleanup
///
/// Cache cleanup is attempted once per session but is only possible if there are no other
/// sessions active (i.e. we can take an exclusive lock on the cache root). If we can,
/// then we check the `.metadata` file and use various strategies to see if the cache
/// folder is stale, including:
///
/// - Doing nothing if it is younger than 1 week old (to avoid churn when switching
///   between CRAN and dev versions repeatedly)
/// - Deleting if the libpath it originated from no longer exists
/// - Deleting if the package it originated from no longer exists
/// - Deleting if the DESCRIPTION it originated from has changed
///
/// [`get`]: PackageSourcesCache::get
/// [`clean`]: PackageSourcesCache::clean
pub struct PackageSourcesCache {
    /// Path to `Rscript`
    r_script_path: PathBuf,

    /// Set of R library paths
    r_libpaths: Vec<PathBuf>,

    /// On disk cache directory root
    cache_root: file_lock::Filesystem,

    /// Shared lock on the root `.lock`, held for the life of this cache.
    ///
    /// Blocks any other process from acquiring the root exclusive lock (the only
    /// thing that can delete entries). That way, any PathBuf we hand out remains
    /// valid for the life of this cache (as long as `PackageSourcesCache` itself
    /// is not dropped!).
    _root_lock: FileLock,

    /// In memory cache to avoid repeated lookups, particularly for packages we've already
    /// determined have no sources.
    cache: HashMap<String, Status>,
}

enum Status {
    NoSources,
    Sources(PathBuf),
}

/// Completion sentinel for a cache entry, written last. Also used to determine if we can
/// clean the cache folder out.
#[derive(Serialize, Deserialize)]
struct Metadata {
    package: String,
    libpath: PathBuf,
    description_hash: String,
    generated_at: DateTime<Utc>,
}

impl PackageSourcesCache {
    pub fn new(r_script_path: PathBuf, r_libpaths: Vec<PathBuf>) -> anyhow::Result<Self> {
        let cache_root = file_lock::Filesystem::new(crate::fs::sources_dir()?);
        cache_root.create_dir()?;

        // Try to clean the cache. Only works if no other processes hold a shared root lock.
        if let Some(cache_root_lockfile) = cache_root.try_open_rw_exclusive_create(LOCK_FILENAME)? {
            if let Err(err) = Self::clean(&cache_root_lockfile) {
                log::warn!("Failed to clean sources cache: {err:?}");
            }
            drop(cache_root_lockfile);
        }

        // Take shared lock for the lifetime of the cache so any paths we hand out stay valid
        let root_lock = cache_root.open_ro_shared_create(LOCK_FILENAME)?;

        Ok(Self {
            r_script_path,
            r_libpaths,
            cache_root,
            _root_lock: root_lock,
            cache: HashMap::new(),
        })
    }

    /// Get a package's cached source folder
    ///
    /// May spawn an R subprocess or download from CRAN (in a blocking manner) to
    /// generate the sources, so keep that in mind when calling this.
    pub fn get(&mut self, package: &str) -> Option<PathBuf> {
        match self.get_impl(package) {
            Ok(Some(sources)) => Some(sources),
            Ok(None) => None,
            Err(err) => {
                log::error!("Failed to get sources for {package}: {err:?}");
                None
            },
        }
    }

    fn get_impl(&mut self, package: &str) -> anyhow::Result<Option<PathBuf>> {
        // Find install path of the package
        let mut libpath = None;
        for r_libpath in &self.r_libpaths {
            if r_libpath.join(package).exists() {
                libpath = Some(r_libpath);
                break;
            }
        }
        let Some(libpath) = libpath else {
            // Not even installed. We don't record anything about this package in
            // the in memory cache in case the user installs it later in the session.
            return Ok(None);
        };

        let package_path = libpath.join(package);
        let namespace_path = package_path.join("NAMESPACE");
        let description_path = package_path.join("DESCRIPTION");

        let description_contents = read_to_string(&description_path)?;
        let description = Description::parse(&description_contents)?;

        let version = description.version.as_str();

        let libpath_hash = hash(libpath.to_string_lossy().as_ref());
        let description_hash = hash(&description_contents);

        // Flat key unique enough to handle:
        // - The same R package across multiple libpaths
        // - Reinstalling a dev R package without changing the version (0.1.0.9000)
        let key =
            format!("{package}_{version}_libpath-{libpath_hash}_description-{description_hash}");

        // In memory cache check
        if let Some(status) = self.cache.get(&key) {
            match status {
                Status::NoSources => return Ok(None),
                Status::Sources(sources) => return Ok(Some(sources.clone())),
            }
        }

        let destination = self.cache_root.join(&key);
        // Safety: We hold the root lock
        let destination_path = destination.as_path_unlocked();

        // Read path: completion sentinel present, already exists on disk
        if destination_path.join(METADATA_FILENAME).exists() {
            self.cache
                .insert(key.clone(), Status::Sources(destination_path.to_path_buf()));
            return Ok(Some(destination_path.to_path_buf()));
        }

        // Write path: take per-key exclusive lock
        destination.create_dir()?;
        let destination_lock = destination.open_rw_exclusive_create(LOCK_FILENAME)?;

        // Re-check: another writer may have populated the key while we waited for an
        // exclusive lock
        if destination_path.join(METADATA_FILENAME).exists() {
            self.cache
                .insert(key.clone(), Status::Sources(destination_path.to_path_buf()));
            return Ok(Some(destination_path.to_path_buf()));
        }

        // Wipe any partial content from a prior writer that may have crashed before
        // writing `.metadata`.
        destination_lock.remove_siblings()?;

        if self.try_populate(
            package,
            version,
            libpath,
            &namespace_path,
            &description_path,
            &description,
            &description_hash,
            destination_path,
        )? {
            self.cache
                .insert(key.clone(), Status::Sources(destination_path.to_path_buf()));
            Ok(Some(destination_path.to_path_buf()))
        } else {
            self.cache.insert(key, Status::NoSources);
            Ok(None)
        }
    }

    /// Writes `DESCRIPTION`, `NAMESPACE`, and `R/` to the cache entry, if possible
    ///
    /// Can assume that `destination_path` exists and we have exclusive access to via the
    /// lock.
    fn try_populate(
        &self,
        package: &str,
        version: &str,
        libpath: &Path,
        namespace_path: &Path,
        description_path: &Path,
        description: &Description,
        description_hash: &str,
        destination_path: &Path,
    ) -> anyhow::Result<bool> {
        if !self.write_r_files(package, version, description, destination_path)? {
            return Ok(false);
        }

        crate::fs::copy_as_readonly(description_path, destination_path.join("DESCRIPTION"))?;
        crate::fs::copy_as_readonly(namespace_path, destination_path.join("NAMESPACE"))?;

        // Last! Only write `.metadata` if all other writes succeed. It is our completion sentinal.
        self.write_metadata(package, libpath, description_hash, destination_path)?;

        Ok(true)
    }

    fn write_r_files(
        &self,
        package: &str,
        version: &str,
        description: &Description,
        destination_path: &Path,
    ) -> anyhow::Result<bool> {
        // Try caching from srcref
        match crate::srcref::cache_srcref(
            package,
            version,
            destination_path,
            &self.r_script_path,
            &self.r_libpaths,
        ) {
            Ok(true) => {
                log::trace!("Cached {package} {version} from srcrefs.");
                return Ok(true);
            },
            Ok(false) => {
                // Fall through
            },
            Err(err) => {
                // Fall through with log
                log::warn!("Failed to cache {package} {version} from srcrefs: {err:?}");
            },
        }

        // Try caching from CRAN
        if matches!(description.repository, Some(Repository::CRAN)) {
            match crate::cran::cache_cran(package, version, destination_path) {
                Ok(true) => {
                    log::trace!("Cached {package} {version} from CRAN download.");
                    return Ok(true);
                },
                Ok(false) => {
                    // Fall through
                },
                Err(err) => {
                    // Fall through with log
                    log::warn!("Failed to cache {package} {version} from CRAN download: {err:?}");
                },
            }
        }

        Ok(false)
    }

    /// Writes the `.metadata` completion sentinel last.
    ///
    /// A reader that sees `.metadata` can trust the rest of the entry is complete
    /// and stable (Other files are read only. Only `clean()` could remove them,
    /// and `clean()` needs the root exclusive lock which no one can take while we
    /// hold a shared lock).
    fn write_metadata(
        &self,
        package: &str,
        libpath: &Path,
        description_hash: &str,
        destination_path: &Path,
    ) -> anyhow::Result<()> {
        let metadata = Metadata {
            package: package.to_string(),
            libpath: libpath.to_path_buf(),
            description_hash: description_hash.to_string(),
            generated_at: Utc::now(),
        };
        let contents = serde_json::to_vec_pretty(&metadata)?;
        std::fs::write(destination_path.join(METADATA_FILENAME), contents)?;
        Ok(())
    }

    /// Walks all cache entries and evicts ones that are provably stale.
    ///
    /// Caller must hold the root exclusive lock.
    fn clean(cache_root_lockfile: &file_lock::FileLock) -> anyhow::Result<()> {
        let root = cache_root_lockfile.parent();
        let now = Utc::now();

        for entry in std::fs::read_dir(root)? {
            let entry = entry?;
            let path = entry.path();

            if !entry.file_type()?.is_dir() {
                // i.e. `.lock` itself
                continue;
            }

            let metadata_path = path.join(METADATA_FILENAME);

            let Ok(metadata_contents) = std::fs::read_to_string(&metadata_path) else {
                log::warn!(
                    "Cleaning {} due to missing or unreadable metadata",
                    path.display()
                );
                crate::fs::remove_dir_all_or_warn(&path);
                continue;
            };

            let metadata: Metadata = match serde_json::from_str(&metadata_contents) {
                Ok(m) => m,
                Err(err) => {
                    log::warn!(
                        "Cleaning {} due to unreadable metadata: {err:?}",
                        path.display()
                    );
                    crate::fs::remove_dir_all_or_warn(&path);
                    continue;
                },
            };

            // Refuse to do anything if younger than 1 week. The user may be switching
            // between CRAN and dev, and we want to keep the cache for the CRAN version
            // around.
            let age = now.signed_duration_since(metadata.generated_at);
            if age < ONE_WEEK {
                continue;
            }

            if !metadata.libpath.exists() {
                log::trace!("Cleaning {} due to nonexistent libpath", path.display());
                crate::fs::remove_dir_all_or_warn(&path);
                continue;
            }

            let package_path = metadata.libpath.join(&metadata.package);

            if !package_path.exists() {
                log::trace!("Cleaning {} due to nonexistent package", path.display());
                crate::fs::remove_dir_all_or_warn(&path);
                continue;
            }

            let Ok(description_contents) =
                std::fs::read_to_string(package_path.join("DESCRIPTION"))
            else {
                log::trace!("Cleaning {} due to missing DESCRIPTION", path.display());
                crate::fs::remove_dir_all_or_warn(&path);
                continue;
            };

            if hash(&description_contents) != metadata.description_hash {
                log::trace!("Cleaning {} due to changed DESCRIPTION", path.display());
                crate::fs::remove_dir_all_or_warn(&path);
                continue;
            }
        }

        Ok(())
    }
}

/// Retain 8 ASCII characters for each hash fragment
fn hash(contents: &str) -> String {
    let mut hash = hex::encode(Sha256::digest(contents));
    hash.truncate(8);
    hash
}
