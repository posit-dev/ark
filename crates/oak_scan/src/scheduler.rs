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
//!   lands in `OrphanRoot` via [`crate::DbScan::upsert_editor`]. When the
//!   scan applies, `upsert_root_file()` finds the existing entity,
//!   promotes it into the right container, and leaves its contents
//!   alone (the buffer wins over the disk read).
//!
//! - **didClose during scan.** The orphan entity moves to stale. The
//!   scan's `upsert_root_file` then resurrects it from stale, restoring
//!   the disk contents the scanner read.
//!
//! - **Watcher events during scan.** R-file events for a pending root
//!   get buffered here and replayed after the scan applies. DESCRIPTION
//!   events flip the root into [`ScanState::ScanningWithRescanQueued`]
//!   so a follow-up scan kicks off after the current one finishes, the
//!   buffered events ride along until the root is finally idle, then
//!   drain in one batch.
//!
//! - **Stale results.** If the workspace folder is removed while its
//!   scan is in flight, the result arrives carrying a `Root` that's no
//!   longer in `workspace_roots`. [`ScanScheduler::apply_scan_completed`]
//!   silently drops it. The `Root` salsa entity stays as a leak (no
//!   GC), but nothing references it.
//!
//! - **Folder added back during in-flight scan.** Each add mints a
//!   fresh `Root`, not a revival of the staled one. That way the
//!   first scan's stale result drops on identity check instead of
//!   clobbering the second scan's empty state.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;

use aether_path::FilePath;
use oak_db::Db;
use oak_db::DbInputs;
use oak_db::Package;
use oak_db::Root;
use oak_db::RootKind;
use salsa::Setter;

use crate::inputs::FileEntry;
use crate::inputs::RootExt;
use crate::packages::scan_workspace_packages;
use crate::packages::scan_workspace_scripts;
use crate::packages::PackageEntry;
use crate::watch::add_watched_file;
use crate::watch::remove_watched_file;
use crate::watch::FileEvent;
use crate::watch::FileEventKind;

/// One scan unit the caller should dispatch.
///
/// Returned from every [`ScanScheduler`] method that can kick off a scan. The
/// caller calls [`ScanRequest::run`] on a worker thread and ships the
/// [`ScanCompleted`] back to [`ScanScheduler::apply_scan_completed`].
#[derive(Clone, Debug)]
#[must_use = "scan requests are dispatched by the caller"]
pub struct ScanRequest {
    pub root: Root,
    pub path: PathBuf,
}

impl ScanRequest {
    /// Run the scan synchronously. No db access, safe to call from any
    /// thread. Production drivers run this on a task pool; tests call
    /// it directly.
    pub fn run(self) -> ScanCompleted {
        let packages = scan_workspace_packages(&self.path);
        let scripts = scan_workspace_scripts(&self.path);
        ScanCompleted {
            root: self.root,
            packages,
            scripts,
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
}

impl ScanCompleted {
    /// Push the scan's output into salsa inputs for `self.root`.
    ///
    /// Atomic full-replacement of the root's packages and scripts.
    /// Existing `File` and `Package` entities are reused by URL where
    /// possible (see [`RootExt::set_package`] /
    /// [`RootExt::set_workspace_scripts`]), so a rescan that doesn't
    /// actually change anything is a no-op as far as downstream salsa
    /// caches are concerned.
    fn apply<DB: Db + DbInputs>(self, db: &mut DB) {
        let ScanCompleted {
            root,
            packages,
            scripts,
        } = self;

        let package_entities: Vec<Package> = packages
            .into_iter()
            .map(|pkg| {
                root.set_package(
                    db,
                    pkg.description_path,
                    pkg.name,
                    pkg.version,
                    pkg.namespace,
                    pkg.files,
                    pkg.scripts,
                    pkg.collation,
                )
            })
            .collect();

        root.set_packages(db).to(package_entities);
        root.set_workspace_scripts(db, scripts);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScanState {
    Scanning,
    ScanningWithRescanQueued,
}

/// Coordinator for asynchronous workspace scanning.
///
/// Tracks which roots have a scan in flight, buffers R-file watcher
/// events for those roots, and coalesces follow-up scan requests. See
/// the module docs for the race-handling design.
#[derive(Debug, Default)]
pub struct ScanScheduler {
    state: HashMap<Root, ScanState>,
    buffered: HashMap<Root, Vec<FileEvent>>,
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
    /// [`RootExt::set_stale`].
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
        let new: Vec<(PathBuf, FilePath)> = paths
            .iter()
            .filter_map(|p| Some((p.clone(), FilePath::from_path_buf(p.clone())?)))
            .collect();
        let new_urls: HashSet<FilePath> = new.iter().map(|(_, u)| u.clone()).collect();

        let old: HashMap<FilePath, Root> = db
            .workspace_roots()
            .roots(db)
            .iter()
            .map(|r| (r.path(db).clone(), *r))
            .collect();

        for (old_url, &old_root) in &old {
            if !new_urls.contains(old_url) {
                old_root.set_stale(db, Some(editor_owned));
                self.state.remove(&old_root);
                self.buffered.remove(&old_root);
            }
        }

        let mut new_roots = Vec::with_capacity(new.len());
        let mut requests = Vec::new();
        for (path, url) in new {
            let root = match old.get(&url) {
                Some(&r) => r,
                None => {
                    let root = Root::new(db, url, RootKind::Workspace, Vec::new(), Vec::new());
                    self.state.insert(root, ScanState::Scanning);
                    requests.push(ScanRequest { root, path });
                    root
                },
            };
            new_roots.push(root);
        }
        db.workspace_roots().set_roots(db).to(new_roots);

        requests
    }

    /// Apply a batch of file-watcher events.
    ///
    /// Per-event routing:
    /// - `DESCRIPTION` events trigger a rescan of the containing root.
    ///   If the root is idle, it transitions to `Scanning` and a
    ///   [`ScanRequest`] is returned. If it's already pending, it
    ///   transitions to `ScanningWithRescanQueued` and the queued
    ///   scan kicks off when `apply_scan_completed()` runs.
    /// - R-file events for an idle root apply surgically (the watcher's
    ///   single-file fast path).
    /// - R-file events for a pending root are buffered and replayed
    ///   after the scan applies, so they don't get dropped against an
    ///   empty `Root`.
    /// - URLs in `skip` are left alone, letting drivers defer to an
    ///   in-memory source of truth (e.g. the editor's open buffers).
    pub fn apply_watcher_events<DB: Db + DbInputs>(
        &mut self,
        db: &mut DB,
        events: Vec<FileEvent>,
        skip: &HashSet<FilePath>,
    ) -> Vec<ScanRequest> {
        let roots = workspace_root_paths(db);
        let mut requests = Vec::new();

        // Pass 1: DESCRIPTION events. Mark each affected root as needing a
        // rescan before any R-file event runs, so an R-file event in the
        // same batch correctly sees the root as pending and buffers
        // instead of applying surgically against a transient world.
        let mut description_roots: HashSet<Root> = HashSet::new();
        for event in &events {
            let Some(path) = event.path.to_path_buf() else {
                continue;
            };
            if path.file_name().is_some_and(|name| name == "DESCRIPTION") {
                if let Some(root) = roots
                    .iter()
                    .find(|(root_path, _)| path.starts_with(root_path))
                    .map(|(_, root)| *root)
                {
                    description_roots.insert(root);
                }
            }
        }
        for root in description_roots {
            if let Some(req) = self.request_rescan(db, root) {
                requests.push(req);
            }
        }

        // Pass 2: R-file events.
        for event in events {
            let Some(path) = event.path.to_path_buf() else {
                continue;
            };
            if path.file_name().is_some_and(|name| name == "DESCRIPTION") {
                continue;
            }
            if skip.contains(&event.path) {
                continue;
            }

            let root = roots
                .iter()
                .find(|(root_path, _)| path.starts_with(root_path))
                .map(|(_, root)| *root);

            match root {
                Some(root) if self.state.contains_key(&root) => {
                    self.buffered.entry(root).or_default().push(event);
                },
                // No in-flight scan for this root: apply the event directly,
                // the watcher's single-file fast path.
                _ => match event.kind {
                    FileEventKind::Created | FileEventKind::Changed => {
                        match std::fs::read_to_string(&path) {
                            Ok(contents) => add_watched_file(db, event.path, contents),
                            Err(err) => {
                                log::warn!("Skipped watched file {}: {err:?}", path.display())
                            },
                        }
                    },
                    FileEventKind::Deleted => remove_watched_file(db, event.path),
                },
            }
        }

        requests
    }

    /// Apply a [`ScanCompleted`] produced by [`ScanRequest::run`].
    ///
    /// Drops the result silently if `result.root` is no longer in
    /// `workspace_roots` (the user removed the folder while its scan was in
    /// flight). Otherwise updates the root's packages and scripts, then handles
    /// the post-apply state:
    ///
    /// - `Scanning`: state cleared, buffered events drained through
    ///   `apply_watcher_events()` (which may itself return new requests).
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
            // Workspace folder removed while we were scanning. Drop the
            // result and any buffered events for this root.
            self.state.remove(&root);
            self.buffered.remove(&root);
            return Vec::new();
        }

        result.apply(db);

        let prior = self.state.remove(&root);
        match prior {
            Some(ScanState::ScanningWithRescanQueued) => {
                // A rescan was queued mid-scan. Resolve its path before
                // re-marking the root `Scanning`: a path we can't resolve must
                // not leave the root `Scanning` with no scan in flight, or it
                // would stay pending forever. On success the buffer rides along
                // and replays when the requeued scan finishes. On failure we
                // fall back to the idle drain.
                match root.path(db).to_path_buf() {
                    Some(path) => {
                        self.state.insert(root, ScanState::Scanning);
                        vec![ScanRequest { root, path }]
                    },
                    None => self.drain_buffered(db, root, editor_owned),
                }
            },
            // We're now idle. Drain any buffered events through the normal path.
            Some(ScanState::Scanning) => self.drain_buffered(db, root, editor_owned),
            None => {
                // A completion for a root we weren't tracking as scanning.
                // Every dispatched scan marks its root `Scanning`, so reaching
                // here means our state diverged from the in-flight work. The
                // result is already applied. Since buffered events only
                // accumulate against a tracked root, there's nothing to drain.
                log::warn!(
                    "Applied a `ScanCompleted` for an untracked root: {path:?}",
                    path = root.path(db)
                );
                Vec::new()
            },
        }
    }

    /// Replay the watcher events buffered for `root` while its scan was in
    /// flight, now that the root is idle. Routes them through
    /// [`Self::apply_watcher_events`], which may itself return fresh requests.
    fn drain_buffered<DB: Db + DbInputs>(
        &mut self,
        db: &mut DB,
        root: Root,
        editor_owned: &HashSet<FilePath>,
    ) -> Vec<ScanRequest> {
        match self.buffered.remove(&root) {
            Some(buffered) => self.apply_watcher_events(db, buffered, editor_owned),
            None => Vec::new(),
        }
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
                let Some(path) = root.path(db).to_path_buf() else {
                    log::warn!("Skipping rescan: root path is not a filesystem path");
                    return None;
                };
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

fn workspace_root_paths<DB: Db + DbInputs>(db: &DB) -> Vec<(PathBuf, Root)> {
    db.workspace_roots()
        .roots(db)
        .iter()
        .filter_map(|root| {
            let Some(path) = root.path(db).to_path_buf() else {
                log::warn!("Skipping workspace root: path is not a filesystem path");
                return None;
            };
            Some((path, *root))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use oak_db::OakDatabase;

    use super::*;

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
        let root = Root::new(&db, path, RootKind::Workspace, Vec::new(), Vec::new());
        db.workspace_roots().set_roots(&mut db).to(vec![root]);

        let mut scheduler = ScanScheduler::new();
        scheduler
            .state
            .insert(root, ScanState::ScanningWithRescanQueued);

        let result = ScanCompleted {
            root,
            packages: Vec::new(),
            scripts: Vec::new(),
        };
        let requests = scheduler.apply_scan_completed(&mut db, result, &HashSet::new());

        assert!(requests.is_empty());
        assert!(!scheduler.has_pending_scans());
    }
}
