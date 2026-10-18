//! Async-friendly coordinator for workspace scanning.
//!
//! Architecture: serial main loop, off-loop async scanners. Scheduler
//! state and salsa inputs are mutated only on the main loop, one event
//! at a time. The expensive scan work (filesystem walk, DESCRIPTION
//! parsing, R file reads) runs on a task pool. The two never touch the
//! same data, so the race-handling below is about event *ordering*,
//! not concurrent access. Same shape `rust-analyzer` and `ty` use to
//! keep `initialize` and `didChangeWorkspaceFolders` from blocking the
//! editor.
//!
//! [`ScanScheduler`] owns the *policy* (when to scan, how to handle
//! events arriving mid-scan), not the runtime. Drivers take a
//! [`ScanRequest`] from the scheduler, run it via [`ScanRequest::run`]
//! on whatever task pool they like, and hand the [`ScanCompleted`] to
//! [`ScanScheduler::apply_scan_completed`]. Tests do the same on the
//! current thread via `drain_scheduler`.
//!
//! # Race surface
//!
//! Three things race once scans are async: the in-flight scan, watcher
//! events for files inside that scan's root, and editor events. They're
//! handled like this:
//!
//! - **didOpen / didChange during scan.** No special handling. The file
//!   lands in `OrphanRoot` via [`crate::DbScan::upsert_editor()`]. When the
//!   scan applies, [`FileUpserts`] reuses the entity through the identity
//!   map and establishes root or package membership without changing its
//!   editor override. The buffer remains authoritative.
//!
//! - **didClose during scan.** The entity leaves `OrphanRoot` with its
//!   override cleared. The scan reuses it through the identity map and
//!   establishes root or package membership. `File::source_text()` then
//!   reads from disk.
//!
//! - **Watcher events during scan.** R-file paths stay buffered until every
//!   containing root is idle, then are reconciled against the disk. Paths
//!   under unrelated idle roots apply without waiting. Discovery events queue
//!   follow-up scans via [`ScanState::ScanningWithRescanQueued`], keeping
//!   affected paths buffered until those scans finish.
//!
//! - **Stale results.** If the workspace folder is removed while its
//!   scan is in flight, the result arrives carrying a `Root` that's no
//!   longer in `workspace_roots`. [`ScanScheduler::apply_scan_completed`]
//!   silently drops it. The `Root` salsa entity stays as a leak (no
//!   GC), but nothing references it.
//!
//! - **Folder added back during in-flight scan.** Each add creates a fresh
//!   `Root`. The identity check rejects the first scan's result so it cannot
//!   overwrite the new root's state.

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::PathBuf;

use aether_path::FilePath;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use indexmap::IndexSet;
use oak_db::Db;
use oak_db::DbInputs;
use oak_db::File;
use oak_db::Package;
use oak_db::Root;
use oak_db::RootKind;
use salsa::Setter;

use crate::eviction::root_files;
use crate::inputs::upsert_package;
use crate::inputs::upsert_workspace_scripts;
use crate::inputs::FileEntry;
use crate::inputs::FileUpserts;
use crate::inputs::RootExt;
use crate::packages::is_environment_dir;
use crate::packages::is_environment_sentinel;
use crate::packages::scan_workspace_environment_dirs;
use crate::packages::scan_workspace_packages;
use crate::packages::scan_workspace_scripts;
use crate::packages::PackageEntry;
use crate::watch::add_watched_file;
use crate::watch::remove_watched_file;

/// One scan unit the caller should dispatch.
///
/// Returned from every [`ScanScheduler`] method that can kick off a scan. The
/// caller calls [`ScanRequest::run`] on a worker thread and ships the
/// [`ScanCompleted`] back to [`ScanScheduler::apply_scan_completed`].
#[derive(Clone, Debug)]
#[must_use = "scan requests are dispatched by the caller"]
pub struct ScanRequest {
    pub root: Root,
    pub path: Utf8PathBuf,
}

impl ScanRequest {
    /// Run the scan synchronously. No db access, safe to call from any
    /// thread. Production drivers run this on a task pool; tests call
    /// it directly.
    pub fn run(self) -> ScanCompleted {
        let packages = scan_workspace_packages(self.path.as_std_path());
        let scripts = scan_workspace_scripts(self.path.as_std_path());
        let environment_dirs = scan_workspace_environment_dirs(self.path.as_std_path());
        ScanCompleted {
            root: self.root,
            packages,
            scripts,
            environment_dirs,
        }
    }
}

/// Output of [`ScanRequest::run`]. Opaque payload carried from the scan back
/// to [`ScanScheduler::apply_scan_completed`].
#[derive(Debug)]
pub struct ScanCompleted {
    root: Root,
    packages: Vec<PackageEntry>,
    scripts: Vec<FileEntry>,
    environment_dirs: Vec<FilePath>,
}

impl ScanCompleted {
    /// Push the scan's output into salsa inputs for `self.root`.
    ///
    /// Unchanged rescans preserve downstream Salsa caches by reusing `File`
    /// and `Package` entities and writing inputs only when they differ. See
    /// [`FileUpserts`] for how `File` entities are matched.
    ///
    /// A path the scan lists in several containers ends up in the last one
    /// written: workspace scripts win over package files (the script walk runs
    /// after the package walk, so it saw the later disk state), and among
    /// packages the one listed last wins. Across nested roots, a package
    /// listed by another root wins instead (see [`upsert_workspace_scripts()`]).
    fn apply<DB: Db + DbInputs>(self, db: &mut DB) {
        let ScanCompleted {
            root,
            packages,
            scripts,
            environment_dirs,
        } = self;

        let mut upserts = FileUpserts::new();

        let package_entities: Vec<Package> = packages
            .into_iter()
            .map(|pkg| {
                // Workspace packages never have an `INDEX`
                let index_revision = None;

                upsert_package(
                    db,
                    &mut upserts,
                    pkg.description_path,
                    pkg.name,
                    pkg.description_revision,
                    pkg.namespace_revision,
                    index_revision,
                    pkg.files,
                    pkg.scripts,
                )
            })
            .collect();

        if root.packages(db) != &package_entities {
            root.set_packages(db).to(package_entities);
        }
        upsert_workspace_scripts(db, &mut upserts, root, scripts);
        upserts.finish(db);

        if root.environment_dirs(db) != &environment_dirs {
            root.set_environment_dirs(db).to(environment_dirs);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScanState {
    Scanning,
    ScanningWithRescanQueued,
}

/// Coordinator for asynchronous workspace scanning.
///
/// Tracks which roots have a scan in flight, buffers the paths of R-file
/// watcher events for those roots, and coalesces follow-up scan requests.
/// See the module docs for the race-handling design.
#[derive(Debug, Default)]
pub struct ScanScheduler {
    state: HashMap<Root, ScanState>,
    /// Unique paths in arrival order. `add_watched_file()` appends new files to
    /// `pkg.files` and `root.scripts` in drain order, and a `HashSet` would make
    /// that order vary from run to run.
    buffered: IndexSet<FilePath>,
}

impl ScanScheduler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether any scan is currently in flight.
    pub fn has_pending_scans(&self) -> bool {
        !self.state.is_empty()
    }

    /// Reconcile `WorkspaceRoots` to exactly `paths`. Returns one
    /// [`ScanRequest`] per newly-added path; unchanged paths skip the
    /// rescan, removed paths are evicted via
    /// [`RootExt::evict()`]. A surviving nested root that shared packages
    /// or scripts with a removed root also gets a rescan, to restore them.
    ///
    /// New roots are inserted into `workspace_roots` empty so watcher
    /// events can find them while their scan is in flight (the events
    /// land in [`ScanScheduler`]'s buffer until the scan applies).
    pub fn set_workspace_paths<DB: Db + DbInputs>(
        &mut self,
        db: &mut DB,
        paths: &[PathBuf],
        editor_owned: &HashSet<FilePath>,
    ) -> Vec<ScanRequest> {
        let new: Vec<(Utf8PathBuf, FilePath)> = paths
            .iter()
            .filter_map(|path| {
                let path = FilePath::from_path_buf(path.clone())?;
                let scan_path = path.as_path()?.to_path_buf();
                Some((scan_path, path))
            })
            .collect();
        let new_paths: HashSet<FilePath> = new.iter().map(|(_, path)| path.clone()).collect();

        let old: HashMap<FilePath, Root> = db
            .workspace_roots()
            .roots(db)
            .iter()
            .map(|r| (r.path(db).clone(), *r))
            .collect();

        let removed: Vec<Root> = old
            .iter()
            .filter(|(old_path, _)| !new_paths.contains(*old_path))
            .map(|(_, &old_root)| old_root)
            .collect();

        // Snapshot before evicting anything: evicting one removed root empties
        // the packages it shares with another.
        let mut evicted_packages: HashSet<Package> = HashSet::new();
        let mut evicted_files: HashSet<File> = HashSet::new();
        for &old_root in &removed {
            evicted_packages.extend(old_root.packages(db).iter().copied());
            evicted_files.extend(root_files(db, old_root));
        }
        for &old_root in &removed {
            old_root.evict(db, Some(editor_owned));
            self.state.remove(&old_root);
        }

        let mut new_roots = Vec::with_capacity(new.len());
        let mut requests = Vec::new();
        for (scan_path, path) in new {
            let root = match old.get(&path) {
                Some(&r) => r,
                None => {
                    let root = Root::new(
                        db,
                        path,
                        RootKind::Workspace,
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                    );
                    self.state.insert(root, ScanState::Scanning);
                    requests.push(ScanRequest {
                        root,
                        path: scan_path,
                    });
                    root
                },
            };
            new_roots.push(root);
        }
        if db.workspace_roots().roots(db) != &new_roots {
            db.workspace_roots().set_roots(db).to(new_roots);
        }

        // Nested roots share `Package` and `File` entities, and `evict()`
        // empties all of a removed root's packages and clears its files'
        // backpointers even when a surviving nested root still lists them. The
        // two roots can hold a file in different containers (a package's
        // `files` in one, `scripts` in the other), so every container counts.
        // Rescanning such a survivor restores them. Survivors that shared
        // nothing keep their state, since a rescan walks the whole root.
        if !removed.is_empty() {
            for (old_path, &old_root) in &old {
                if !new_paths.contains(old_path) {
                    continue;
                }
                let shares = old_root
                    .packages(db)
                    .iter()
                    .any(|pkg| evicted_packages.contains(pkg)) ||
                    root_files(db, old_root)
                        .iter()
                        .any(|file| evicted_files.contains(file));
                if shares {
                    requests.extend(self.request_rescan(db, old_root));
                }
            }
        }

        self.drain_buffered(db, editor_owned);

        requests
    }

    /// Apply a batch of file-watcher events, given as the paths they report.
    ///
    /// The kind of change (created, changed, deleted) isn't needed: each path
    /// is reconciled against the disk when it applies, which also absorbs
    /// events that are coalesced, duplicated, or out of date by then.
    ///
    /// `DESCRIPTION`, `.Rprofile`, and `.Renviron` paths rescan every
    /// containing root, including when the editor owns them. Sentinel events
    /// that leave an idle root's `environment_dirs()` accurate are skipped
    /// (see `is_environment_dir_current()`). Idle roots return a
    /// [`ScanRequest`]. Roots with an in-flight scan queue a rescan for
    /// `apply_scan_completed()` to start.
    ///
    /// R-file paths apply directly only when no containing root has a pending
    /// scan. Otherwise they are buffered until all containing roots finish
    /// scanning, so older scan results cannot overwrite their effects. Paths
    /// in `editor_owned` are never removed, so an open buffer survives the
    /// deletion of its file on disk.
    pub fn apply_watcher_events<DB: Db + DbInputs>(
        &mut self,
        db: &mut DB,
        paths: Vec<FilePath>,
        editor_owned: &HashSet<FilePath>,
    ) -> Vec<ScanRequest> {
        let roots = workspace_root_paths(db);
        let mut requests = Vec::new();

        // Request rescans before handling R-file events in the same batch,
        // so those events buffer instead of being overwritten by scan results.
        //
        // Rescan every containing root because nested roots keep separate
        // `packages` and `environment_dirs` lists for overlapping paths.
        // Ignore `editor_owned` here because discovery depends on disk state
        // even when the editor owns a file's contents.
        let mut rescan_roots: Vec<Root> = Vec::new();
        let mut listings = DirListings::default();
        for path in &paths {
            let Some(fs_path) = path.as_path() else {
                continue;
            };
            if !triggers_rescan(fs_path) {
                continue;
            }
            for (root_path, root) in &roots {
                if !fs_path.starts_with(root_path) || rescan_roots.contains(root) {
                    continue;
                }
                if self.is_environment_dir_current(db, *root, fs_path, &mut listings) {
                    continue;
                }
                rescan_roots.push(*root);
            }
        }
        for root in rescan_roots {
            if let Some(req) = self.request_rescan(db, root) {
                requests.push(req);
            }
        }

        // Reconcile only this batch's paths. Already-buffered paths are still
        // blocked: only a finished scan or a change of roots can unblock them,
        // and those drain the whole buffer. Draining it here too would revisit
        // the backlog on every batch that arrives during a long scan.
        let r_paths: IndexSet<FilePath> = paths
            .into_iter()
            .filter(|path| {
                path.as_path()
                    .is_some_and(|fs_path| !triggers_rescan(fs_path))
            })
            .collect();
        for path in r_paths {
            if reconcile_watched_path(db, &self.state, &roots, &mut listings, editor_owned, &path) {
                self.buffered.insert(path);
            }
        }

        requests
    }

    /// Whether a sentinel event on `path` leaves environment discovery unchanged.
    ///
    /// Its parent must remain in `root.environment_dirs()`, contain a sentinel,
    /// and be reachable below the root without symlinks and under its stored
    /// spelling. A symlink or a case rename would change the next scan's
    /// listing even if the sentinel still resolves (see [`is_walkable_dir()`]).
    ///
    /// Only the existence of `.Rprofile` and `.Renviron` affects scans, so
    /// edits to them are skipped. `DESCRIPTION` always rescans because its
    /// `Package:` field matters. The check counts every sentinel in the
    /// directory, so deleting `.Rprofile` next to a `.Renviron` is skipped too.
    ///
    /// An unlisted directory without a sentinel isn't treated as current, even
    /// though it looks consistent. `environment_dirs()` matches paths exactly,
    /// so a deletion reported under another spelling of a listed directory
    /// (case or Unicode normalization) would look unlisted and be skipped.
    ///
    /// A root with a scan in flight is never current: its `environment_dirs()`
    /// predates the scan, whose result may disagree with the disk by the time
    /// it applies.
    fn is_environment_dir_current<DB: Db + DbInputs>(
        &self,
        db: &DB,
        root: Root,
        path: &Utf8Path,
        listings: &mut DirListings,
    ) -> bool {
        if self.state.contains_key(&root) {
            return false;
        }
        if !path.file_name().is_some_and(is_environment_sentinel) {
            return false;
        }
        let Some(dir) = path.parent() else {
            return false;
        };
        let Some(root_path) = root.path(db).as_path() else {
            return false;
        };
        let listed = root
            .environment_dirs(db)
            .iter()
            .any(|env_dir| env_dir.as_path() == Some(dir));
        listed && is_environment_dir(dir.as_std_path()) && is_walkable_dir(root_path, dir, listings)
    }

    /// Apply a [`ScanCompleted`] produced by [`ScanRequest::run`].
    ///
    /// Drops the result silently if `result.root` is no longer in
    /// `workspace_roots` (the user removed the folder while its scan was in
    /// flight). Otherwise updates the root's packages and scripts, then handles
    /// the post-apply state:
    ///
    /// - `Scanning`: state cleared, buffered events applied if no other
    ///   containing root has a pending scan.
    /// - `ScanningWithRescanQueued`: fresh `ScanRequest` returned, state stays
    ///   `Scanning`, buffer preserved for the next round.
    /// - Untracked (`None`): unexpected, since dispatch always marks the root
    ///   `Scanning`. Logged as a warning, the result is still applied.
    pub fn apply_scan_completed<DB: Db + DbInputs>(
        &mut self,
        db: &mut DB,
        result: ScanCompleted,
        editor_owned: &HashSet<FilePath>,
    ) -> Vec<ScanRequest> {
        let root = result.root;

        let live = db.workspace_roots().roots(db).contains(&root);
        if !live {
            // Workspace reconciliation already re-evaluated buffered events
            // against the surviving roots; this result must not touch them.
            self.state.remove(&root);
            return Vec::new();
        }

        result.apply(db);
        orphan_dropped_editor_files(db, editor_owned);
        self.finish_scan(db, root, editor_owned)
    }

    /// Release `root` from scanning after its task panics or is dropped without
    /// producing a result.
    ///
    /// No scan result replaces the root's contents. Buffered watcher events can
    /// still apply once all containing roots are idle. A rescan queued by events
    /// during the failed scan runs first, keeping those paths buffered.
    /// Failure alone does not trigger a retry, avoiding an endless retry loop
    /// for a scan that consistently panics.
    pub fn apply_scan_failed<DB: Db + DbInputs>(
        &mut self,
        db: &mut DB,
        root: Root,
        editor_owned: &HashSet<FilePath>,
    ) -> Vec<ScanRequest> {
        let live = db.workspace_roots().roots(db).contains(&root);
        if !live {
            self.state.remove(&root);
            return Vec::new();
        }
        self.finish_scan(db, root, editor_owned)
    }

    fn finish_scan<DB: Db + DbInputs>(
        &mut self,
        db: &mut DB,
        root: Root,
        editor_owned: &HashSet<FilePath>,
    ) -> Vec<ScanRequest> {
        let prior = self.state.remove(&root);
        let requests = match prior {
            Some(ScanState::ScanningWithRescanQueued) => {
                // A rescan was queued mid-scan. Resolve its path before
                // re-marking the root `Scanning`: a path we can't resolve must
                // not leave the root `Scanning` with no scan in flight, or it
                // would stay pending forever. On success the buffer rides along
                // and replays when the requeued scan finishes. On failure we
                // fall back to the idle drain.
                let scan_path = root.path(db).as_path().map(Utf8Path::to_path_buf);
                match scan_path {
                    Some(path) => {
                        self.state.insert(root, ScanState::Scanning);
                        vec![ScanRequest { root, path }]
                    },
                    None => Vec::new(),
                }
            },
            Some(ScanState::Scanning) => Vec::new(),
            None => {
                // Every dispatched scan records a scanning state, so its absence
                // means scheduler state diverged from the in-flight work. On the
                // success path, the result has already been applied.
                log::warn!(
                    "Finished a scan for an untracked root: {path:?}",
                    path = root.path(db)
                );
                Vec::new()
            },
        };
        self.drain_buffered(db, editor_owned);
        requests
    }

    /// Apply buffered paths whose containing roots are all idle.
    ///
    /// Files are added or refreshed. Missing paths, non-files, and stale case
    /// spellings are removed (see [`is_file_on_disk()`]). Repeated events for
    /// one path collapse into its current disk state, so they need no ordered replay.
    ///
    /// Paths in `editor_owned` are added but never removed: the open buffer
    /// stays the source of truth until it closes. Adding them matters for a
    /// buffer opened before its file existed on disk, or rescued to `OrphanRoot`
    /// and then saved. Without it the file would stay out of every container,
    /// and become invisible when the buffer closes.
    ///
    /// Editor ownership is checked only once a path is unblocked. A blocked
    /// path must survive a buffer that opens and closes before its scan
    /// completes, otherwise the scan's older listing would win.
    fn drain_buffered<DB: Db + DbInputs>(&mut self, db: &mut DB, editor_owned: &HashSet<FilePath>) {
        let roots = workspace_root_paths(db);
        let mut listings = DirListings::default();
        let state = &self.state;
        self.buffered.retain(|path| {
            reconcile_watched_path(db, state, &roots, &mut listings, editor_owned, path)
        });
    }

    fn request_rescan<DB: Db + DbInputs>(
        &mut self,
        db: &mut DB,
        root: Root,
    ) -> Option<ScanRequest> {
        match self.state.get(&root) {
            Some(ScanState::Scanning) => {
                self.state.insert(root, ScanState::ScanningWithRescanQueued);
                None
            },
            Some(ScanState::ScanningWithRescanQueued) => None,
            None => {
                let Some(path) = root.path(db).as_path() else {
                    log::warn!("Skipping rescan: root path is not a filesystem path");
                    return None;
                };
                let path = path.to_path_buf();
                self.state.insert(root, ScanState::Scanning);
                Some(ScanRequest { root, path })
            },
        }
    }
}

/// Run every request on the current thread, feeding results back
/// until no further rescans are queued.
///
/// Test-only: production drivers spawn each request on a task pool
/// so the LSP handler doesn't block. Crate-private and `cfg(test)`
/// so this can't leak into a production caller; out-of-crate tests
/// that need a synchronous drainer write their own 3-line loop over
/// [`ScanRequest::run`] + [`ScanScheduler::apply_scan_completed`].
#[cfg(test)]
pub(crate) fn drain_scheduler<DB: Db + DbInputs>(
    db: &mut DB,
    scheduler: &mut ScanScheduler,
    mut requests: Vec<ScanRequest>,
    editor_owned: &HashSet<FilePath>,
) {
    while let Some(req) = requests.pop() {
        let result = req.run();
        requests.extend(scheduler.apply_scan_completed(db, result, editor_owned));
    }
}

/// These files affect package or environment discovery, which the single-file
/// watcher path cannot recompute.
fn triggers_rescan(path: &Utf8Path) -> bool {
    path.file_name()
        .is_some_and(|name| name == "DESCRIPTION" || is_environment_sentinel(name))
}

/// Apply one watched R-file path against the disk, unless a containing root is
/// scanning. Returns `true` if the path must stay buffered.
///
/// See [`ScanScheduler::drain_buffered()`] for how open buffers are handled.
fn reconcile_watched_path<DB: Db + DbInputs>(
    db: &mut DB,
    state: &HashMap<Root, ScanState>,
    roots: &[(Utf8PathBuf, Root)],
    listings: &mut DirListings,
    editor_owned: &HashSet<FilePath>,
    path: &FilePath,
) -> bool {
    let Some(fs_path) = path.as_path() else {
        return false;
    };
    let containing_roots: Vec<&(Utf8PathBuf, Root)> = roots
        .iter()
        .filter(|(root_path, _)| fs_path.starts_with(root_path))
        .collect();
    let Some((root_path, _)) = containing_roots.first() else {
        return false;
    };

    // Nested roots share entities, so every containing scan must finish
    // before a path can apply.
    if containing_roots
        .iter()
        .any(|(_, root)| state.contains_key(root))
    {
        return true;
    }
    let on_disk = match is_file_on_disk(root_path, fs_path, listings) {
        Ok(on_disk) => on_disk,
        // `NotADirectory` means an ancestor was replaced by a regular
        // file (`ENOTDIR` on Unix), so the path can't exist either.
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            false
        },
        // Other errors (e.g. permissions) don't prove the file is gone,
        // so keep the current registration rather than unlinking it.
        Err(err) => {
            log::warn!("Can't check watched path {fs_path} on disk: {err:?}");
            return false;
        },
    };
    if on_disk {
        add_watched_file(db, path.clone());
    } else if !editor_owned.contains(path) {
        remove_watched_file(db, path.clone());
    }
    false
}

/// Reject stale ASCII case spellings of files below `root_path`.
///
/// On case-insensitive volumes, `fs::metadata()` still resolves `foo.R` after
/// a rename to `Foo.R`. Accepting both spellings would leave two registered
/// [`File`] entities for one disk file. Directory listings expose the stored
/// spelling, so a path component is stale when its parent lists an ASCII case
/// variant but no exact match.
///
/// A component with neither an exact match nor an ASCII case variant is accepted
/// if metadata resolves the file. APFS also ignores Unicode normalization, so
/// an NFC client name can resolve to an NFD disk name. Requiring an exact match
/// would unlink that file. This can retain duplicates, but avoids removing a
/// file that still exists. The client-supplied spelling of `root_path` itself
/// is not checked.
fn is_file_on_disk(
    root_path: &Utf8Path,
    fs_path: &Utf8Path,
    listings: &mut DirListings,
) -> io::Result<bool> {
    if !fs::metadata(fs_path)?.is_file() {
        return Ok(false);
    }
    Ok(!has_stale_spelling(root_path, fs_path, listings)?)
}

/// Whether a component of `path` below `root_path` is a stale ASCII case
/// spelling: its parent lists a case variant of it but no exact match. See
/// [`is_file_on_disk()`] for why other mismatches are accepted.
fn has_stale_spelling(
    root_path: &Utf8Path,
    path: &Utf8Path,
    listings: &mut DirListings,
) -> io::Result<bool> {
    let Ok(relative) = path.strip_prefix(root_path) else {
        return Ok(false);
    };

    let mut dir = root_path.to_path_buf();
    for component in relative.components() {
        let name = component.as_str();
        let listing = listings.get(&dir)?;
        let name_os = OsStr::new(name);
        if listing.folded.contains(&name_os.to_ascii_lowercase()) &&
            !listing.exact.contains(name_os)
        {
            return Ok(true);
        }
        dir.push(name);
    }
    Ok(false)
}

/// Cache directory listings for one pass over watcher events: one batch in
/// [`ScanScheduler::apply_watcher_events()`] or one
/// [`ScanScheduler::drain_buffered()`].
///
/// Reading an N-entry directory for each of N buffered paths would visit N^2
/// entries on the main loop. Sharing the listing avoids repeated directory
/// reads, and indexing by ASCII-lowercased name avoids repeated linear searches.
///
/// Listings are discarded after the pass. Disk changes during the pass can make
/// them stale, but subsequent watcher events are checked against fresh listings.
#[derive(Default)]
struct DirListings(HashMap<Utf8PathBuf, DirListing>);

/// Entry names of one directory, as stored and ASCII-lowercased.
struct DirListing {
    exact: HashSet<OsString>,
    folded: HashSet<OsString>,
}

impl DirListings {
    fn get(&mut self, dir: &Utf8Path) -> io::Result<&DirListing> {
        match self.0.entry(dir.to_path_buf()) {
            Entry::Occupied(entry) => Ok(entry.into_mut()),
            Entry::Vacant(entry) => Ok(entry.insert(read_listing(dir)?)),
        }
    }
}

fn read_listing(dir: &Utf8Path) -> io::Result<DirListing> {
    let mut exact = HashSet::new();
    let mut folded = HashSet::new();
    for entry in fs::read_dir(dir)? {
        let entry_name = entry?.file_name();
        folded.insert(entry_name.to_ascii_lowercase());
        exact.insert(entry_name);
    }
    Ok(DirListing { exact, folded })
}

/// Whether every component of `dir` below `root_path` is a real directory,
/// under the spelling stored on disk.
///
/// The scan's walker doesn't follow symlinks below the root, so a listed
/// directory replaced by a symlink (or reached through one) drops out of the
/// next scan even though `is_environment_dir()` still finds its sentinel
/// through the link.
///
/// The walker also reports stored spellings. After `Sub` is renamed to `sub`
/// on a case-insensitive volume, `Sub` still resolves but the next scan lists
/// `sub` instead. An unreadable listing counts as not walkable.
fn is_walkable_dir(root_path: &Utf8Path, dir: &Utf8Path, listings: &mut DirListings) -> bool {
    let Ok(relative) = dir.strip_prefix(root_path) else {
        return false;
    };
    if !matches!(has_stale_spelling(root_path, dir, listings), Ok(false)) {
        return false;
    }
    let mut current = root_path.to_path_buf();
    for component in relative.components() {
        current.push(component);
        let Ok(metadata) = fs::symlink_metadata(&current) else {
            return false;
        };
        if !metadata.is_dir() {
            return false;
        }
    }
    true
}

/// Keep editor-owned files reachable through
/// [`OrphanRoot`](oak_db::OrphanRoot) when a scan removes them from every live
/// container, as happens when their files are missing on disk. Otherwise
/// `file_by_path()` would miss an entity still held by the editor and analysis
/// would lose the buffer.
///
/// Files still reachable after the scan retain their container and package,
/// including entities shared with another root. Calling `set_package()` on a
/// shared `Package` can remove files from another root's view, so every
/// editor-owned path is checked, not just those under the scanned root.
///
/// Every lookup runs before the first write, so the orphan path index isn't
/// invalidated and rebuilt once per rescued file.
fn orphan_dropped_editor_files<DB: Db + DbInputs>(db: &mut DB, editor_owned: &HashSet<FilePath>) {
    let mut dropped: HashSet<File> = HashSet::new();
    for path in editor_owned {
        let Some(file) = db.interned_file(path) else {
            continue;
        };
        match db.file_by_path(path) {
            None => {
                dropped.insert(file);
            },
            Some(current) => stdext::soft_assert!(
                current == file,
                "A second `File` is published for an open buffer: {path}"
            ),
        }
    }
    if dropped.is_empty() {
        return;
    }

    for &file in &dropped {
        if file.package(db).is_some() {
            file.set_package(db).to(None);
        }
    }
    let orphan = db.orphan_root();
    let mut orphan_files = orphan.files(db).clone();
    orphan_files.extend(dropped.iter().copied());
    orphan.set_files(db).to(orphan_files);
}

fn workspace_root_paths<DB: Db + DbInputs>(db: &DB) -> Vec<(Utf8PathBuf, Root)> {
    db.workspace_roots()
        .roots(db)
        .iter()
        .filter_map(|root| {
            let Some(path) = root.path(db).as_path() else {
                log::warn!("Skipping workspace root: path is not a filesystem path");
                return None;
            };
            Some((path.to_path_buf(), *root))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use oak_db::OakDatabase;
    use oak_db::SourceDb;

    use super::*;
    use crate::DbScan;

    /// A root whose URL has no filesystem path (e.g. an `untitled:` buffer)
    /// can't produce a `ScanRequest`. When such a root is
    /// `ScanningWithRescanQueued` and its scan completes, the scheduler must
    /// not leave it `Scanning` with nothing in flight, or it would stay pending
    /// forever. This state is unreachable through the public API (workspace
    /// roots always come from `from_path_buf`), so we build it by hand.
    #[test]
    fn test_unresolvable_rescan_path_does_not_strand_root_scanning() {
        let mut db = OakDatabase::new();
        let path = FilePath::parse("untitled:Untitled-1").unwrap();
        let root = Root::new(
            &db,
            path,
            RootKind::Workspace,
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        db.workspace_roots().set_roots(&mut db).to(vec![root]);

        let mut scheduler = ScanScheduler::new();
        scheduler
            .state
            .insert(root, ScanState::ScanningWithRescanQueued);

        let result = ScanCompleted {
            root,
            packages: Vec::new(),
            scripts: Vec::new(),
            environment_dirs: Vec::new(),
        };
        let requests = scheduler.apply_scan_completed(&mut db, result, &HashSet::new());

        assert!(requests.is_empty());
        assert!(!scheduler.has_pending_scans());
    }

    // `DESCRIPTION` can disappear between the package walk and the script walk
    // of one scan, so both list the same file. It must end up as a single
    // `File`, placed as a workspace script, whether it starts live, absent or
    // evicted. The race can't be timed through `ScanRequest::run()`, so these
    // tests build its result by hand.

    #[test]
    fn test_live_path_listed_as_package_file_and_script_keeps_one_entity() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = OakDatabase::new();
        let mut scheduler = ScanScheduler::new();
        let requests =
            scheduler.set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());
        drain_scheduler(&mut db, &mut scheduler, requests, &HashSet::new());

        let path = write_package_with_file(tmp.path());
        let file = db.upsert_editor(path.clone(), "edited <- 1\n".to_string());
        let editor_owned = HashSet::from([path.clone()]);

        let description = FilePath::from_path_buf(tmp.path().join("pkg/DESCRIPTION")).unwrap();
        let requests = scheduler.apply_watcher_events(&mut db, vec![description], &editor_owned);
        let root = apply_listed_twice(&mut db, &mut scheduler, requests, &editor_owned);

        let pkg = root.packages(&db)[0];
        assert!(pkg.files(&db).is_empty());
        assert_eq!(root.scripts(&db).clone(), vec![file]);
        assert_eq!(file.package(&db), None);
        assert_eq!(db.file_by_path(&path), Some(file));
        assert_eq!(file.source_text(&db), "edited <- 1\n");
        assert!(!db.orphan_root().files(&db).contains(&file));
    }

    #[test]
    fn test_new_path_listed_as_package_file_and_script_gets_one_entity() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = OakDatabase::new();
        let mut scheduler = ScanScheduler::new();
        let path = write_package_with_file(tmp.path());

        let requests =
            scheduler.set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());
        let root = apply_listed_twice(&mut db, &mut scheduler, requests, &HashSet::new());

        let pkg = root.packages(&db)[0];
        assert!(pkg.files(&db).is_empty());
        let scripts = root.scripts(&db).clone();
        assert_eq!(scripts.len(), 1);
        assert_eq!(scripts[0].package(&db), None);
        assert_eq!(db.file_by_path(&path), Some(scripts[0]));
    }

    #[test]
    fn test_evicted_path_listed_as_package_file_and_script_reuses_one_entity() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = OakDatabase::new();
        let mut scheduler = ScanScheduler::new();
        let path = write_package_with_file(tmp.path());
        let workspace = [tmp.path().to_path_buf()];
        let requests = scheduler.set_workspace_paths(&mut db, &workspace, &HashSet::new());
        drain_scheduler(&mut db, &mut scheduler, requests, &HashSet::new());
        let file = db.file_by_path(&path).unwrap();

        let requests = scheduler.set_workspace_paths(&mut db, &[], &HashSet::new());
        assert!(requests.is_empty());
        assert!(db.file_by_path(&path).is_none());

        let requests = scheduler.set_workspace_paths(&mut db, &workspace, &HashSet::new());
        let root = apply_listed_twice(&mut db, &mut scheduler, requests, &HashSet::new());

        let pkg = root.packages(&db)[0];
        assert!(pkg.files(&db).is_empty());
        assert_eq!(root.scripts(&db).clone(), vec![file]);
        assert_eq!(file.package(&db), None);
        assert_eq!(db.file_by_path(&path), Some(file));
    }

    /// Write `pkg/` with a `DESCRIPTION` and `R/a.R` under `dir`. Returns the
    /// path of `R/a.R`.
    fn write_package_with_file(dir: &std::path::Path) -> FilePath {
        let pkg_dir = dir.join("pkg");
        fs::create_dir_all(pkg_dir.join("R")).unwrap();
        fs::write(
            pkg_dir.join("DESCRIPTION"),
            "Package: pkg\nVersion: 0.0.0\n",
        )
        .unwrap();
        fs::write(pkg_dir.join("R/a.R"), "x <- 1\n").unwrap();
        FilePath::from_path_buf(pkg_dir.join("R/a.R")).unwrap()
    }

    /// Complete the single scan in `requests` with a result that lists the
    /// package's files both in the package and as workspace scripts. Returns
    /// the scanned root.
    fn apply_listed_twice(
        db: &mut OakDatabase,
        scheduler: &mut ScanScheduler,
        mut requests: Vec<ScanRequest>,
        editor_owned: &HashSet<FilePath>,
    ) -> Root {
        assert_eq!(requests.len(), 1);
        let request = requests.remove(0);
        let packages = scan_workspace_packages(request.path.as_std_path());
        assert_eq!(packages.len(), 1);
        let scripts = packages[0].files.clone();
        let root = request.root;
        let result = ScanCompleted {
            root,
            packages,
            scripts,
            environment_dirs: Vec::new(),
        };
        let requests = scheduler.apply_scan_completed(db, result, editor_owned);
        assert!(requests.is_empty());
        root
    }
}
