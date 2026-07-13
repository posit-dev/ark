//! A generic, content-agnostic on disk directory cache
//!
//! A [`Cache`] is rooted at a single subfolder of the shared cache directory. Each entry
//! is a directory keyed by a caller-supplied string and filled by a caller-supplied
//! `populate` closure. The cache knows nothing about what lives inside an entry. The
//! lock, completion-sentinel, last-access, and eviction machinery all live here so
//! callers don't reimplement it.
//!
//! There are two primary ways to evict an entry:
//! - An entry untouched for [`DEFAULT_AGE`] is dropped.
//! - When the total number of entries exceeds [`DEFAULT_CAPACITY`], the least recently
//!   touched entries are dropped.

mod file_lock;

use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::SystemTime;

use crate::file_lock::FileLock;
use crate::file_lock::Filesystem;

/// Name of the root lock file and the per-key lock file.
const LOCK_FILENAME: &str = ".lock";

/// Default max age for a cache entry
///
/// Roughly 4 months in seconds, with 30 days per month
const DEFAULT_AGE: Duration = Duration::from_secs(4 * 30 * 24 * 60 * 60);

/// Default max number of cache entries
const DEFAULT_CAPACITY: usize = 1000;

/// Name of the completion sentinel, written last in each cache entry.
///
/// An entry without it is a crashed partial and is removed by [`Cache::clean`].
///
/// Its mtime doubles as the entry's last-access time. [`Cache::get`] bumps it
/// on every retrieval (this is an atomic operation so we allow it even though
/// we'll only hold a shared lock on this key's folder). [`Cache::clean`] uses
/// the mtime both to evict entries older than [`DEFAULT_AGE`] and to evict the
/// oldest first when the total number of entries exceeds [`DEFAULT_CAPACITY`].
const COMPLETE_FILENAME: &str = ".complete";

/// A generic on disk directory cache
///
/// # On disk layout
///
/// The cache is rooted at one subfolder of `<cache_dir>/oak/`. Each entry is a
/// directory keyed by the caller:
///
/// ```text
/// <cache_dir>/oak/<root>/
///     .lock
///     <key>/
///         .lock
///         .complete
///         <content>
/// ```
///
/// `.complete` is written last and is the completion sentinel.
///
/// # Locking
///
/// The root `.lock` can be held shared or exclusive:
///
/// - **Shared** is held for the lifetime of this `Cache`. While held, entries can be read
///   and new entries appended, but nothing can be deleted. Multiple sessions can hold it
///   simultaneously. It keeps any [`PathBuf`] handed out by [`Cache::get`] valid for the
///   life of the cache.
///
/// - **Exclusive** is only attempted once, at [`Cache::open`], to run [`Cache::clean`].
///   It is skipped if another session already holds the shared lock, in which case we
///   just try again next time.
///
/// Each entry also has its own `<key>/.lock`, taken exclusively in [`Cache::insert`] so
/// two sessions never populate the same key at once.
///
/// This mirrors cargo's model, which faces the same multi-process cache challenge.
#[derive(Debug)]
pub struct Cache {
    /// On disk root of this cache's subfolder.
    root: Filesystem,

    /// Shared lock on the root `.lock`, held for the life of this `Cache`.
    ///
    /// Blocks any other process from taking the root exclusive lock (the only thing
    /// that deletes entries), so any [`PathBuf`] we hand out stays valid as long as
    /// this `Cache` lives.
    root_lock: FileLock,
}

impl Cache {
    /// Open `<cache_dir>/oak/<root>/`, creating it if needed.
    ///
    /// Runs a best-effort [`Cache::clean`] under the exclusive root lock (skipped if
    /// another session holds the shared lock), then holds the shared root lock for the
    /// life of the returned `Cache` so handed-out paths stay valid.
    pub fn open(root: &str) -> anyhow::Result<Self> {
        Self::open_in(cache_dir()?.join(root))
    }

    /// Like [`Cache::open`], but rooted at an explicit `root` rather than a subfolder of
    /// the shared cache directory. Only useful for testing against a temp directory.
    pub fn open_in(root: PathBuf) -> anyhow::Result<Self> {
        Self::open_with_options(root, DEFAULT_AGE, DEFAULT_CAPACITY)
    }

    /// Like [`Cache::open_in`], but with explicit eviction thresholds rather than
    /// [`DEFAULT_AGE`] and [`DEFAULT_CAPACITY`]. Useful for tests.
    fn open_with_options(root: PathBuf, age: Duration, capacity: usize) -> anyhow::Result<Self> {
        let root = Filesystem::new(root);
        root.create_dir()?;

        // Try to clean. Only possible if no other session holds the shared root lock.
        if let Some(root_lock) = root.try_open_rw_exclusive_create(LOCK_FILENAME)? {
            if let Err(err) = clean(&root_lock, age, capacity) {
                log::warn!(
                    "Failed to clean cache at {root}: {err:?}",
                    root = root.display()
                );
            }
            drop(root_lock);
        }

        // Hold the shared root lock for life so handed-out paths stay valid.
        let root_lock = root.open_ro_shared_create(LOCK_FILENAME)?;

        Ok(Self { root, root_lock })
    }

    /// Returns the entry directory for `key` if it is present and complete, refreshing
    /// its last-access time. Cheap! No locking, just a sentinel check.
    pub fn get(&self, key: &str) -> Option<PathBuf> {
        let dir = self.root_lock.parent().join(key);
        if !dir.join(COMPLETE_FILENAME).exists() {
            return None;
        }
        bump_last_access(&dir);
        Some(dir)
    }

    /// Returns the entry directory for `key`, populating it if absent.
    ///
    /// - Takes the per-key exclusive lock
    /// - Double-checks completion (another session may have populated it while we waited
    ///   for the exclusive lock)
    /// - Wipes any crashed partial results
    /// - Runs `populate`
    /// - Writes the completion sentinel
    ///
    /// `populate` returns `Ok(true)` if it filled the entry, or `Ok(false)` if the
    /// content is genuinely unavailable (e.g. the package isn't on CRAN, or there are no
    /// srcrefs). A `false` results in no `.complete` entry. This returns `Ok(None)` and
    /// any partial result is removed by the next [`Cache::clean`].
    pub fn insert(
        &self,
        key: &str,
        populate: impl FnOnce(&Path) -> anyhow::Result<bool>,
    ) -> anyhow::Result<Option<PathBuf>> {
        let key = self.root.join(key);
        key.create_dir()?;

        // Take exclusive lock on per-key folder to avoid contention with another writer
        let lock = key.open_rw_exclusive_create(LOCK_FILENAME)?;
        let dir = lock.parent().to_path_buf();

        // Another writer may have completed this key while we waited for the lock
        if dir.join(COMPLETE_FILENAME).exists() {
            bump_last_access(&dir);
            return Ok(Some(dir));
        }

        // Wipe any partial content from a prior writer that crashed before completing
        lock.remove_siblings()?;

        if !populate(&dir)? {
            // If we fail to populate, clean up any stray files. Can't clean up the
            // folder here because we don't have an exclusive lock (can create but not
            // delete!), but since we don't write `.complete` the folder will be deleted
            // on the next `clean()`.
            lock.remove_siblings()?;
            return Ok(None);
        }

        // Last! `.complete` is the completion sentinel, so it must follow `populate()`.
        std::fs::write(dir.join(COMPLETE_FILENAME), b"")?;

        Ok(Some(dir))
    }
}

/// Removes crashed partials, evicts entries older than `age`, then trims to `capacity`
///
/// The caller must hold the root exclusive lock, which no one can take while a live
/// session holds the shared lock, so eviction can never race a reader.
fn clean(root_lock: &FileLock, age: Duration, capacity: usize) -> anyhow::Result<()> {
    let now = SystemTime::now();
    let root = root_lock.parent();

    let mut entries = Vec::new();

    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();

        if entry.file_name().as_os_str() == LOCK_FILENAME {
            // We own `.lock`
            continue;
        }

        // Cache entries are always directories. If, by some chance, we find something
        // else, that's a mistake, so remove it.
        if !entry.file_type()?.is_dir() {
            log::trace!("Evicting stray cache file {}", path.display());
            remove_file_or_warn(&path);
            continue;
        }

        let complete = path.join(COMPLETE_FILENAME);
        if !complete.exists() {
            log::trace!("Evicting incomplete cache entry {}", path.display());
            remove_dir_all_or_warn(&path);
            continue;
        }

        let accessed = last_access(&complete);

        // Evict entries not accessed within `age`. Treat pathological future `accessed`
        // times as stale.
        let stale = now
            .duration_since(accessed)
            .map_or(true, |duration_since| duration_since > age);

        if stale {
            log::trace!("Evicting stale cache entry {}", path.display());
            remove_dir_all_or_warn(&path);
            continue;
        }

        entries.push((path, accessed));
    }

    // Evict the oldest until `capacity` remain
    if entries.len() > capacity {
        entries.sort_by_key(|(_, accessed)| *accessed);
        let excess = entries.len() - capacity;
        for (path, _) in entries.into_iter().take(excess) {
            log::trace!(
                "Evicting cache entry exceeding cache capacity {}",
                path.display()
            );
            remove_dir_all_or_warn(&path);
        }
    }

    Ok(())
}

/// Bumps an entry's last-access time by touching the `.complete` sentinel's mtime.
///
/// Best-effort: failure is non-fatal, the entry just keeps its previous mtime and may
/// be evicted sooner than it should. If two processes try to update this at the same
/// time, that's fine, last writer wins on an atomic OS operation. The more important
/// thing is that we hold a shared lock on the cache, so a clean can't remove this while
/// we update it.
fn bump_last_access(dir: &Path) {
    if let Err(err) = std::fs::File::options()
        .write(true)
        .open(dir.join(COMPLETE_FILENAME))
        .and_then(|file| file.set_modified(SystemTime::now()))
    {
        log::trace!(
            "Failed to refresh access time for {dir}: {err:?}",
            dir = dir.display()
        );
    }
}

/// Last-access time of an entry, read from its `.complete` sentinel's mtime
///
/// If we can't access it, something is wrong, so we return [SystemTime::UNIX_EPOCH]
/// to make this folder look very old
fn last_access(complete: &Path) -> SystemTime {
    std::fs::metadata(complete)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

fn remove_dir_all_or_warn(dir: &Path) {
    if let Err(err) = std::fs::remove_dir_all(dir) {
        log::warn!(
            "Failed to remove directory {dir}: {err:?}",
            dir = dir.display()
        );
    }
}

fn remove_file_or_warn(path: &Path) {
    if let Err(err) = std::fs::remove_file(path) {
        log::warn!(
            "Failed to remove file {path}: {err:?}",
            path = path.display()
        );
    }
}

/// Base directory for all oak caches: `<cache_dir>/oak/`
fn cache_dir() -> anyhow::Result<PathBuf> {
    use etcetera::BaseStrategy;
    // Can fail if the home directory can't be found
    Ok(etcetera::choose_base_strategy()?.cache_dir().join("oak"))
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;
    use std::time::SystemTime;

    use tempfile::TempDir;

    use crate::Cache;
    use crate::COMPLETE_FILENAME;
    use crate::DEFAULT_AGE;
    use crate::DEFAULT_CAPACITY;

    /// Creates a cache rooted in a fresh temp dir, returning both so the temp dir stays
    /// alive for the test.
    fn new() -> (TempDir, Cache) {
        let dir = TempDir::new().unwrap();
        let cache = Cache::open_in(dir.path().join("subfolder")).unwrap();
        (dir, cache)
    }

    /// A `populate` that writes a single file and reports success.
    fn write_file(contents: &str) -> impl FnOnce(&Path) -> anyhow::Result<bool> + '_ {
        move |dir| {
            std::fs::write(dir.join("content.txt"), contents)?;
            Ok(true)
        }
    }

    /// Forces an entry's last-access time by setting the `.complete` sentinel's mtime.
    fn set_accessed(dir: &Path, time: SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(dir.join(COMPLETE_FILENAME))
            .unwrap()
            .set_modified(time)
            .unwrap();
    }

    fn accessed(dir: &Path) -> SystemTime {
        std::fs::metadata(dir.join(COMPLETE_FILENAME))
            .unwrap()
            .modified()
            .unwrap()
    }

    #[test]
    fn test_get_miss() {
        let (_dir, cache) = new();
        assert_eq!(cache.get("absent"), None);
    }

    #[test]
    fn test_insert_then_get_round_trip() {
        let (_dir, cache) = new();

        let inserted = cache.insert("key", write_file("hello")).unwrap().unwrap();
        let got = cache.get("key").unwrap();
        assert_eq!(inserted, got);
        assert_eq!(
            std::fs::read_to_string(got.join("content.txt")).unwrap(),
            "hello"
        );
    }

    #[test]
    fn test_insert_unavailable_yields_no_entry() {
        let (_dir, cache) = new();

        let result = cache.insert("key", |_dir| Ok(false)).unwrap();
        assert_eq!(result, None);
        assert_eq!(cache.get("key"), None);
    }

    #[test]
    fn test_clean_removes_crashed_partial() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("subfolder");

        // Populate one good entry, then forge a crashed partial (no `.complete`).
        {
            let cache = Cache::open_in(root.clone()).unwrap();
            cache.insert("good", write_file("ok")).unwrap();
        }
        let partial = root.join("partial");
        std::fs::create_dir(&partial).unwrap();
        std::fs::write(partial.join("content.txt"), "junk").unwrap();

        // Reopening runs `clean`, which removes the partial but keeps the good entry.
        let cache = Cache::open_in(root).unwrap();
        assert!(!partial.exists());
        assert!(cache.get("good").is_some());
    }

    #[test]
    fn test_clean_removes_stray_file_but_keeps_lock() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("subfolder");

        {
            let cache = Cache::open_in(root.clone()).unwrap();
            cache.insert("good", write_file("ok")).unwrap();
        }
        // A stray file in the cache root that doesn't belong to any entry.
        let stray = root.join("stray.txt");
        std::fs::write(&stray, "junk").unwrap();

        // Reopening runs `clean`, which removes the stray file but keeps our root
        // `.lock` and the good entry.
        let cache = Cache::open_in(root.clone()).unwrap();
        assert!(!stray.exists());
        assert!(root.join(".lock").exists());
        assert!(cache.get("good").is_some());
    }

    #[test]
    fn test_clean_trims_to_capacity_keeping_recent() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("subfolder");
        let now = SystemTime::now();

        // Insert four entries, then stamp their access times so the ordering is
        // deterministic
        {
            let cache = Cache::open_in(root.clone()).unwrap();
            for (index, key) in ["oldest", "older", "newer", "newest"].iter().enumerate() {
                let entry = cache.insert(key, write_file(key)).unwrap().unwrap();
                set_accessed(&entry, now - Duration::from_secs(4 - index as u64));
            }
        }

        // Reopening with capacity 2 evicts the two least-recently-accessed.
        let cache = Cache::open_with_options(root, DEFAULT_AGE, 2).unwrap();
        assert_eq!(cache.get("oldest"), None);
        assert_eq!(cache.get("older"), None);
        assert!(cache.get("newer").is_some());
        assert!(cache.get("newest").is_some());
    }

    #[test]
    fn test_clean_evicts_stale_entries() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("subfolder");
        let now = SystemTime::now();

        // A fresh entry and an entry last accessed an hour ago
        {
            let cache = Cache::open_in(root.clone()).unwrap();
            let fresh = cache.insert("fresh", write_file("fresh")).unwrap().unwrap();
            set_accessed(&fresh, now);
            let stale = cache.insert("stale", write_file("stale")).unwrap().unwrap();
            set_accessed(&stale, now - Duration::from_secs(3600));
        }

        // Reopening with a half-hour max age evicts the stale entry but keeps the fresh
        // one
        let cache =
            Cache::open_with_options(root, Duration::from_secs(1800), DEFAULT_CAPACITY).unwrap();
        assert!(cache.get("fresh").is_some());
        assert_eq!(cache.get("stale"), None);
    }

    #[test]
    fn test_clean_evicts_future_mtime() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("subfolder");

        // Forge an entry whose access time is in the future, as if the wall clock was
        // reset. It has no meaningful age, so we treat it as broken.
        {
            let cache = Cache::open_in(root.clone()).unwrap();
            let entry = cache
                .insert("future", write_file("future"))
                .unwrap()
                .unwrap();
            set_accessed(&entry, SystemTime::now() + Duration::from_secs(3600));
        }

        // Reopening evicts it
        let cache = Cache::open_with_options(root, DEFAULT_AGE, DEFAULT_CAPACITY).unwrap();
        assert_eq!(cache.get("future"), None);
    }

    #[test]
    fn test_get_refreshes_accessed() {
        let (_dir, cache) = new();

        let entry = cache.insert("key", write_file("hello")).unwrap().unwrap();

        // Backdate the access time, then confirm a `get` hit moves it forward.
        let backdated = SystemTime::now() - Duration::from_secs(3600);
        set_accessed(&entry, backdated);

        cache.get("key").unwrap();

        assert!(accessed(&entry) > backdated);
    }

    #[test]
    fn test_insert_is_complete_after_populate() {
        let (_dir, cache) = new();
        let entry = cache.insert("key", write_file("hello")).unwrap().unwrap();
        assert!(entry.join(COMPLETE_FILENAME).exists());
    }
}
