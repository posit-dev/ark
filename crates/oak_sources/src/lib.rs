mod base;
mod cran;
mod download;
mod fs;
mod hash;
mod installed_package;
mod srcref;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::RwLock;

use chrono::DateTime;
use chrono::TimeDelta;
use chrono::Utc;
use oak_fs::file_lock;
use oak_fs::file_lock::FileLock;
use oak_package::package_description::Priority;
use oak_package::package_description::Repository;
use serde::Deserialize;
use serde::Serialize;

use crate::installed_package::InstalledPackage;

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

/// A cache of extracted R package sources
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
/// The `{package}` and `{version}` components of the key are not required to make it
/// unique, but make for a nicer reading experience when goto-definition opens one of
/// these files in your editor.
///
/// # Locking
///
/// The cache root `.lock` can be locked as shared or exclusive:
///
/// - **Shared root lock** is held for the lifetime of this `PackageCache`. The invariant
///   is that if you hold a shared root lock, you can read from or append new entries to
///   the cache, but never delete from it. Multiple sessions can hold this simultaneously.
///   The main purpose is to prevent cleanup from running so that any PathBuf handed out
///   by [`get`] stays valid while this lock is held.
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
/// This cache design is somewhat similar to cargo's model, except cargo doesn't hold
/// long running shared locks since each cargo command is pretty short lived.
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
/// [`get`]: PackageCache::get
/// [`clean`]: PackageCache::clean
pub struct PackageCache {
    /// Path to `R` binary
    r: PathBuf,

    /// Set of R library paths
    r_libpaths: Vec<PathBuf>,

    /// On disk cache directory root
    cache_root: file_lock::Filesystem,

    /// Shared lock on the root `.lock`, held for the life of this cache.
    ///
    /// Blocks any other process from acquiring the root exclusive lock (the only thing
    /// that can delete entries). That way, any `PathBuf` we hand out remains valid for
    /// the life of this cache (as long as `PackageCache` itself is not dropped!).
    cache_root_lock: FileLock,

    /// Set of packages which are installed, but we failed to populate their sources (from
    /// CRAN or srcrefs). If we request sources for one of these packages a second time,
    /// we don't attempt expensive source generation again.
    ///
    /// Inside an [RwLock] so that [PackageCache::get()] avoids being `mut`, allowing a
    /// caller to wrap a [PackageCache] in an [std::sync::Arc] and call
    /// [PackageCache::get()] in the background on a thread, acting as a form of
    /// "prefetching".
    source_unavailable: RwLock<HashSet<String>>,
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

impl PackageCache {
    pub fn new(r: PathBuf, r_libpaths: Vec<PathBuf>) -> anyhow::Result<Self> {
        let cache_root = file_lock::Filesystem::new(crate::fs::sources_dir()?);
        cache_root.create_dir()?;

        // Try to clean the cache. Only works if no other processes hold a shared root lock.
        if let Some(cache_root_lock) = cache_root.try_open_rw_exclusive_create(LOCK_FILENAME)? {
            if let Err(err) = Self::clean(&cache_root_lock) {
                log::warn!("Failed to clean sources cache: {err:?}");
            }
            drop(cache_root_lock);
        }

        // Take shared lock for the lifetime of the cache so any paths we hand out stay valid
        let cache_root_lock = cache_root.open_ro_shared_create(LOCK_FILENAME)?;

        Ok(Self {
            r,
            r_libpaths,
            cache_root,
            cache_root_lock,
            source_unavailable: RwLock::new(HashSet::new()),
        })
    }

    /// Get a package's cached source folder
    ///
    /// May spawn an R subprocess or download from CRAN (in a blocking manner) to
    /// generate the sources, so keep that in mind when calling this.
    pub fn get(&self, package: &str) -> Option<PathBuf> {
        match self.get_result(package) {
            Ok(Some(sources)) => Some(sources),
            Ok(None) => None,
            Err(err) => {
                log::error!("Failed to get sources for {package}: {err:?}");
                None
            },
        }
    }

    fn get_result(&self, package: &str) -> anyhow::Result<Option<PathBuf>> {
        let Some(package) = InstalledPackage::find(package, &self.r_libpaths)? else {
            // Not even installed
            return Ok(None);
        };

        // Read path: completion sentinel present, already exists on disk
        let destination = self.cache_root_lock.parent().join(package.key());
        if destination.join(METADATA_FILENAME).exists() {
            return Ok(Some(destination));
        }

        // Check if we've already tried to generate sources for this installed package but
        // failed. If so, refuse to attempt expensive source generation again.
        if self
            .source_unavailable
            .read()
            .is_ok_and(|set| set.contains(package.key()))
        {
            return Ok(None);
        }

        // Write path
        let result = if matches!(package.description().priority, Some(Priority::Base)) {
            // R version to download is the same as the base package version
            self.try_populate_base(&package.description().version)
        } else {
            self.try_populate(&package)
        };

        match result {
            Ok(true) => Ok(Some(destination)),
            Ok(false) => {
                // Unavailable for some reason, maybe package isn't on CRAN.
                // Never try and generate sources again this session.
                self.source_unavailable
                    .write()
                    .ok()
                    .map(|mut set| set.insert(package.key().to_string()));
                Ok(None)
            },
            Err(err) => {
                // Errored for some reason during source generation, maybe a download failed.
                // Never try and generate sources again this session.
                log::error!(
                    "Failed to cache {name} {version}: {err:?}",
                    name = package.name(),
                    version = package.version()
                );
                self.source_unavailable
                    .write()
                    .ok()
                    .map(|mut set| set.insert(package.key().to_string()));
                Ok(None)
            },
        }
    }

    fn try_populate_base(&self, version: &str) -> anyhow::Result<bool> {
        // Download the R sources in their entirety
        let Some(bytes) = crate::base::download(version)? else {
            log::trace!("No R source tarball on CRAN for version {version}");
            return Ok(false);
        };

        // Populate all base packages from the download
        for package in crate::base::BASE_PACKAGES {
            let Some(package) = InstalledPackage::find(package, &self.r_libpaths)? else {
                // It would be very odd to not find a base package
                return Ok(false);
            };
            self.try_populate_base_package(&package, version, &bytes)?;
        }

        Ok(true)
    }

    fn try_populate_base_package(
        &self,
        package: &InstalledPackage,
        version: &str,
        bytes: &[u8],
    ) -> anyhow::Result<()> {
        // Take per-key exclusive lock
        let destination = self.cache_root.join(package.key());
        destination.create_dir()?;
        let destination_lock = destination.open_rw_exclusive_create(LOCK_FILENAME)?;

        // Another writer may have populated the key while we waited for an exclusive lock
        if destination_lock.parent().join(METADATA_FILENAME).exists() {
            return Ok(());
        }

        // Wipe any partial content from a prior writer that may have crashed before
        // writing `.metadata`.
        destination_lock.remove_siblings()?;

        crate::base::extract(package.name(), version, bytes, &destination_lock)?;

        crate::fs::copy_as_readonly(
            package.description_path(),
            destination_lock.parent().join("DESCRIPTION"),
        )?;

        // The `base` package itself has no NAMESPACE, for now we generate an empty
        // NAMESPACE, but eventually we will want to fully populate it with a
        // pseudo-NAMESPACE.
        if package.name() == "base" {
            std::fs::write(destination_lock.parent().join("NAMESPACE"), "")?;
            crate::fs::set_readonly(destination_lock.parent().join("NAMESPACE"))?;
        } else {
            crate::fs::copy_as_readonly(
                package.namespace_path(),
                destination_lock.parent().join("NAMESPACE"),
            )?;
        }

        // Last! `.metadata` is the completion sentinel.
        self.write_metadata(package, &destination_lock)?;

        Ok(())
    }

    /// Writes `DESCRIPTION`, `NAMESPACE`, and `R/` to the cache entry, if possible
    fn try_populate(&self, package: &InstalledPackage) -> anyhow::Result<bool> {
        // Take per-key exclusive lock
        let destination = self.cache_root.join(package.key());
        destination.create_dir()?;
        let destination_lock = destination.open_rw_exclusive_create(LOCK_FILENAME)?;

        // Another writer may have populated the key while we waited for an exclusive lock
        if destination_lock.parent().join(METADATA_FILENAME).exists() {
            return Ok(true);
        }

        // Wipe any partial content from a prior writer that may have crashed before
        // writing `.metadata`.
        destination_lock.remove_siblings()?;

        if !self.write_r_files(package, &destination_lock)? {
            return Ok(false);
        }

        crate::fs::copy_as_readonly(
            package.description_path(),
            destination_lock.parent().join("DESCRIPTION"),
        )?;
        crate::fs::copy_as_readonly(
            package.namespace_path(),
            destination_lock.parent().join("NAMESPACE"),
        )?;

        // Last! Only write `.metadata` if all other writes succeed. It is our completion sentinal.
        self.write_metadata(package, &destination_lock)?;

        Ok(true)
    }

    fn write_r_files(
        &self,
        package: &InstalledPackage,
        destination_lock: &FileLock,
    ) -> anyhow::Result<bool> {
        // Try caching from srcref
        match crate::srcref::cache_srcref(
            package.name(),
            &package.description().version,
            destination_lock,
            &self.r,
            &self.r_libpaths,
        ) {
            Ok(true) => {
                log::trace!(
                    "Cached {name} {version} from srcrefs.",
                    name = package.name(),
                    version = package.version()
                );
                return Ok(true);
            },
            Ok(false) => {
                // Fall through
            },
            Err(err) => {
                // Fall through with log
                log::warn!(
                    "Failed to cache {name} {version} from srcrefs: {err:?}",
                    name = package.name(),
                    version = package.version()
                );
            },
        }

        // Try caching from CRAN
        if matches!(package.description().repository, Some(Repository::CRAN)) {
            match crate::cran::cache_cran(package.name(), package.version(), destination_lock) {
                Ok(true) => {
                    log::trace!(
                        "Cached {name} {version} from CRAN download.",
                        name = package.name(),
                        version = package.version()
                    );
                    return Ok(true);
                },
                Ok(false) => {
                    // Fall through
                },
                Err(err) => {
                    // Fall through with log
                    log::warn!(
                        "Failed to cache {name} {version} from CRAN download: {err:?}",
                        name = package.name(),
                        version = package.version()
                    );
                },
            }
        }

        // TODO: Also consider Bioconductor as a source, which seem to have a `biocViews`
        // field in their DESCRIPTION according to some renv docs. We will also need a
        // Bioconductor version to download for. pkgcache has some R code that helps
        // with this kind of thing.
        // https://github.com/rstudio/renv/blob/c689571a0a6ce83a2f82a93b396f3a1ba87b0282/vignettes/package-sources.Rmd#L34-L50
        // https://github.com/r-lib/pkgcache/blob/05d430e907064c5dea822ff82d4257f0b5668070/R/bioc.R

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
        package: &InstalledPackage,
        destination_lock: &FileLock,
    ) -> anyhow::Result<()> {
        let metadata = Metadata {
            package: package.name().to_string(),
            libpath: package.library_path().to_path_buf(),
            description_hash: package.description_hash().to_string(),
            generated_at: Utc::now(),
        };
        let contents = serde_json::to_vec_pretty(&metadata)?;
        std::fs::write(destination_lock.parent().join(METADATA_FILENAME), contents)?;
        Ok(())
    }

    /// Walks all cache entries and evicts ones that are provably stale.
    ///
    /// Caller must hold the root exclusive lock.
    fn clean(cache_root_lock: &file_lock::FileLock) -> anyhow::Result<()> {
        let cache_root = cache_root_lock.parent();
        let now = Utc::now();

        for entry in std::fs::read_dir(cache_root)? {
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

            if crate::hash::hash(&description_contents) != metadata.description_hash {
                log::trace!("Cleaning {} due to changed DESCRIPTION", path.display());
                crate::fs::remove_dir_all_or_warn(&path);
                continue;
            }
        }

        Ok(())
    }
}

// // For local testing
// #[cfg(test)]
// mod tests {
//     use std::path::PathBuf;
//
//     use crate::PackageCache;
//
//     #[test]
//     fn testit() {
//         let r_script_path = PathBuf::from("/usr/local/bin/Rscript");
//         let r_libpaths = vec![
//             PathBuf::from("/Users/davis/Library/R/arm64/4.5/library"),
//             PathBuf::from("/Library/Frameworks/R.framework/Versions/4.5-arm64/Resources/library"),
//         ];
//         let cache = PackageCache::new(r_script_path, r_libpaths).unwrap();
//         cache.get("utils");
//     }
// }
