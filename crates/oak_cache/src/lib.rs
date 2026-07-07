//! A generic, content-agnostic on disk directory cache
//!
//! A [`Cache`] is rooted at a single subfolder of the shared cache directory and
//! holds a fixed number of entries (an LRU `capacity`). Each entry is a directory
//! keyed by a caller-supplied string and filled by a caller-supplied `populate`
//! closure. The cache knows nothing about what lives inside an entry. The lock,
//! completion-sentinel, last-access, and LRU-eviction machinery all live here so
//! callers don't reimplement it.

mod file_lock;

use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;

use crate::file_lock::FileLock;
use crate::file_lock::Filesystem;

/// Name of the root lock file and the per-key lock file.
const LOCK_FILENAME: &str = ".lock";

/// Name of the completion sentinel, written last in each cache entry.
///
/// An entry without it is a crashed partial and is removed by [`Cache::clean`].
///
/// Its mtime doubles as the entry's last-access time. [`Cache::get`] bumps it on every
/// retrieval (this is an atomic operation so we allow it even though we'll only hold a
/// shared lock on this key's folder) and [`Cache::clean`] evicts the entries with the
/// oldest mtime first.
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
    pub fn open(root: &str, capacity: usize) -> anyhow::Result<Self> {
        Self::open_in(cache_dir()?.join(root), capacity)
    }

    /// Like [`Cache::open`], but rooted at an explicit `root` rather than a subfolder of
    /// the shared cache directory. Only useful for testing against a temp directory.
    pub fn open_in(root: PathBuf, capacity: usize) -> anyhow::Result<Self> {
        let root = Filesystem::new(root);
        root.create_dir()?;

        // Try to clean. Only possible if no other session holds the shared root lock.
        if let Some(root_lock) = root.try_open_rw_exclusive_create(LOCK_FILENAME)? {
            if let Err(err) = clean(&root_lock, capacity) {
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

/// Removes crashed partials and trims the cache down to `capacity`
///
/// The caller must hold the root exclusive lock, which no one can take while a live
/// session holds the shared lock, so eviction can never race a reader.
///
/// First removes any entry missing its `.complete` sentinel. Then, if the surviving
/// entries exceed `capacity`, removes the least-recently-accessed ones until `capacity`
/// remain.
fn clean(root_lock: &FileLock, capacity: usize) -> anyhow::Result<()> {
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

        entries.push((path, last_access(&complete)));
    }

    if entries.len() <= capacity {
        return Ok(());
    }

    // Oldest first, then evict down to `capacity`
    entries.sort_by_key(|(_, accessed)| *accessed);
    let excess = entries.len() - capacity;
    for (path, _) in entries.into_iter().take(excess) {
        log::trace!("Evicting old cache entry {}", path.display());
        remove_dir_all_or_warn(&path);
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

    /// Creates a cache rooted in a fresh temp dir, returning both so the temp dir stays
    /// alive for the test.
    fn new(capacity: usize) -> (TempDir, Cache) {
        let dir = TempDir::new().unwrap();
        let cache = Cache::open_in(dir.path().join("subfolder"), capacity).unwrap();
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
        let (_dir, cache) = new(10);
        assert_eq!(cache.get("absent"), None);
    }

    #[test]
    fn test_insert_then_get_round_trip() {
        let (_dir, cache) = new(10);

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
        let (_dir, cache) = new(10);

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
            let cache = Cache::open_in(root.clone(), 10).unwrap();
            cache.insert("good", write_file("ok")).unwrap();
        }
        let partial = root.join("partial");
        std::fs::create_dir(&partial).unwrap();
        std::fs::write(partial.join("content.txt"), "junk").unwrap();

        // Reopening runs `clean`, which removes the partial but keeps the good entry.
        let cache = Cache::open_in(root, 10).unwrap();
        assert!(!partial.exists());
        assert!(cache.get("good").is_some());
    }

    #[test]
    fn test_clean_removes_stray_file_but_keeps_lock() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("subfolder");

        {
            let cache = Cache::open_in(root.clone(), 10).unwrap();
            cache.insert("good", write_file("ok")).unwrap();
        }
        // A stray file in the cache root that doesn't belong to any entry.
        let stray = root.join("stray.txt");
        std::fs::write(&stray, "junk").unwrap();

        // Reopening runs `clean`, which removes the stray file but keeps our root
        // `.lock` and the good entry.
        let cache = Cache::open_in(root.clone(), 10).unwrap();
        assert!(!stray.exists());
        assert!(root.join(".lock").exists());
        assert!(cache.get("good").is_some());
    }

    #[test]
    fn test_clean_trims_to_capacity_keeping_recent() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("subfolder");
        let now = SystemTime::now();

        // Insert four entries under a capacity that won't evict, then stamp their
        // access times so the ordering is deterministic.
        {
            let cache = Cache::open_in(root.clone(), 10).unwrap();
            for (index, key) in ["oldest", "older", "newer", "newest"].iter().enumerate() {
                let entry = cache.insert(key, write_file(key)).unwrap().unwrap();
                set_accessed(&entry, now + Duration::from_secs(index as u64));
            }
        }

        // Reopening with capacity 2 evicts the two least-recently-accessed.
        let cache = Cache::open_in(root, 2).unwrap();
        assert_eq!(cache.get("oldest"), None);
        assert_eq!(cache.get("older"), None);
        assert!(cache.get("newer").is_some());
        assert!(cache.get("newest").is_some());
    }

    #[test]
    fn test_get_refreshes_accessed() {
        let (_dir, cache) = new(10);

        let entry = cache.insert("key", write_file("hello")).unwrap().unwrap();

        // Backdate the access time, then confirm a `get` hit moves it forward.
        let backdated = SystemTime::now() - Duration::from_secs(3600);
        set_accessed(&entry, backdated);

        cache.get("key").unwrap();

        assert!(accessed(&entry) > backdated);
    }

    #[test]
    fn test_insert_is_complete_after_populate() {
        let (_dir, cache) = new(10);
        let entry = cache.insert("key", write_file("hello")).unwrap().unwrap();
        assert!(entry.join(COMPLETE_FILENAME).exists());
    }
}
