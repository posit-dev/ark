//! Update helpers for `oak_db`. Each helper drives one update pattern
//! that a single salsa setter can't safely express on its own.
//!
//! The helpers preserve entity identity across rescans: a `File` is
//! keyed by URL, a `Package` by its `DESCRIPTION` name within its root,
//! and a `Root` by its path. Repeat scans reuse the existing entities
//! and update their fields in place rather than minting new ones. This
//! keeps downstream salsa caches (parse, semantic_index, `Definition`
//! entities for goto-def) stable across changes that don't actually
//! touch a given file's content.
//!
//! The trade-off is a small placement invariant: `file.package` must
//! agree with which container Vec holds the file (`pkg.files`,
//! `root.scripts`, or `orphan_root().files`). Outside callers should
//! not call `file.set_package(...)` directly. This crate is the only
//! intended caller of the placement-affecting setters on `oak_db`'s
//! input structs.

use std::collections::HashSet;
use std::path::PathBuf;

use aether_url::UrlId;
use oak_db::Db;
use oak_db::DbInputs;
use oak_db::File;
use oak_db::Package;
use oak_db::Root;
use oak_package_metadata::namespace::Namespace;
use salsa::Setter;

use crate::lookup::package_by_url;
use crate::stale::remove_from_stale_files;
use crate::stale::remove_from_stale_packages;
use crate::stale::stale_file_by_url;
use crate::watch;
use crate::watch::FileEvent;
use crate::workspace;

/// Description of one R file the scanner wants to register.
///
/// `contents` is the on-disk snapshot at scan time. It's used as the
/// initial content whenever the helper mints a new `File` entity, i.e.
/// the first time a URL is seen, whether at the initial scan or on a
/// later rescan that discovers a newly-created file.
///
/// If a `File` already exists at this URL (scanner-created from an
/// earlier scan, or VFS-created via `didOpen`), the helpers reuse that
/// entity and leave its content alone. `set_contents` (driven by the
/// VFS) is the authoritative way to update content.
#[derive(Clone, Debug)]
pub struct FileEntry {
    pub url: UrlId,
    pub contents: String,
}

/// Extension methods on the database for scanner orchestration and
/// placement-aware updates that don't have a natural `Root` receiver.
pub trait DbScan: Db + DbInputs {
    /// Reconcile `LibraryRoots` to exactly `paths`.
    ///
    /// - Paths already present as a `Root`: untouched. No fs walk, no
    ///   salsa churn.
    ///
    /// - New paths: scanned and added.
    ///
    /// - Removed paths: their `Root` is dropped and the contained `File`
    ///   and `Package` entities move to [`oak_db::StaleRoot`] so that
    ///   a later call that brings the same path back reuses the same
    ///   entities (Salsa never GCs them since they are inputs).
    ///
    /// Order in `LibraryRoots.roots` follows `paths`, matching R's
    /// `.libPaths()` precedence.
    fn set_library_paths(&mut self, paths: &[PathBuf]);

    /// Reconcile `WorkspaceRoots` to exactly `paths`.
    ///
    /// - Paths already present as a `Root`: untouched. No fs walk, no salsa
    ///   churn. The file watcher handles in-folder changes.
    ///
    /// - New paths: scanned (`DESCRIPTION` files at any depth, honouring
    ///   `.gitignore`, plus top-level R scripts) and added.
    ///
    /// - Removed paths: their `Root` is evicted. Files whose URLs are in
    ///   `editor_owned` move to [`oak_db::OrphanRoot`] (analysis-visible: the
    ///   buffer is still open). Everything else moves to [`oak_db::StaleRoot`]
    ///   for entity reuse if the path comes back.
    fn set_workspace_paths(&mut self, paths: &[PathBuf], editor_owned: &HashSet<UrlId>);

    /// Rescan one workspace root. Used as the coarse fallback when
    /// `DESCRIPTION` events change the package classification of a directory.
    fn rescan_workspace_root(&mut self, root: Root);

    /// Upsert the editor's view of a file. Used by the LSP layer to apply
    /// `didOpen` / `didChange` content for any URL the editor touches.
    ///
    /// If a `File` already exists at this URL (in a live root or orphan),
    /// only its contents are updated. Classification is left as-is: a file
    /// the scanner had previously placed in a package stays in that package
    /// (`didOpen` is a content event, not a reclassification).
    ///
    /// If no live `File` exists but one is in [`StaleRoot`] from a prior
    /// [`Self::close_editor`], it gets resurrected into `orphan_root` with
    /// the new content. This ways, reopening a previously-closed buffer reuses
    /// the same `File` input entity in the Salsa cache.
    ///
    /// If no `File` exists at all, one is created in `orphan_root().files`.
    /// It stays there until another handler reclassifies it.
    fn upsert_editor(&mut self, url: UrlId, contents: String) -> File;

    /// Mark the editor as no longer holding a buffer for this URL.
    ///
    /// If the file lives in [`OrphanRoot`] (placed there by
    /// [`Self::upsert_editor`] because the URL didn't belong to a live root, or
    /// by `set_workspace_paths()` eviction routing for an open buffer in a
    /// removed workspace), it gets moved to [`StaleRoot`]. Future
    /// [`Self::upsert_editor`] for the same URL resurrects the entity from
    /// stale instead of minting a fresh one.
    ///
    /// If the file is in a live workspace / library container, the call is a
    /// no-op.
    fn close_editor(&mut self, url: &UrlId);

    /// React to a Created or Changed watcher event on an R file. Classifies the
    /// URL against the current workspace tree and either creates a new `File`
    /// or updates an existing one's content. Files outside every workspace, or
    /// inside a package's non-`R/` subdir, are skipped.
    fn add_watched_file(&mut self, url: UrlId, contents: String);

    /// React to a Deleted watcher event. Unlinks the file from whichever
    /// container holds it (package files, root scripts, or orphan).
    fn remove_watched_file(&mut self, url: UrlId);

    /// Apply a batch of file-watcher events. Routes DESCRIPTION events to a
    /// coarse rescan of the containing workspace root (deduped within the
    /// batch), and R-file events to per-file add / remove. URLs in `editor_owned` are
    /// left alone, so callers can defer to an in-memory source of truth (e.g.
    /// the editor's open buffers).
    fn apply_watcher_events(&mut self, events: Vec<FileEvent>, editor_owned: &HashSet<UrlId>);
}

impl<DB: Db + DbInputs> DbScan for DB {
    fn set_library_paths(&mut self, paths: &[PathBuf]) {
        crate::library::set_library_paths(self, paths);
    }

    fn set_workspace_paths(&mut self, paths: &[PathBuf], editor_owned: &HashSet<UrlId>) {
        crate::workspace::set_workspace_paths(self, paths, editor_owned);
    }

    fn rescan_workspace_root(&mut self, root: Root) {
        workspace::rescan_workspace_root(self, root);
    }

    fn upsert_editor(&mut self, url: UrlId, contents: String) -> File {
        if let Some(existing) = self.file_by_url(&url) {
            existing.set_contents(self).to(contents);
            return existing;
        }

        // Resurrect a previously-closed buffer from stale. The didOpen
        // content overwrites whatever the stale entity carried.
        if let Some(stale) = stale_file_by_url(self, &url) {
            stale.set_contents(self).to(contents);
            stale.set_package(self).to(None);
            remove_from_stale_files(self, stale);
            add_to_orphan_files(self, stale);
            return stale;
        }

        let file = File::new(self, url, contents, None);
        add_to_orphan_files(self, file);
        file
    }

    fn close_editor(&mut self, url: &UrlId) {
        let Some(file) = self.file_by_url(url) else {
            return;
        };

        let orphan = self.orphan_root();
        if !orphan.files(self).contains(&file) {
            // A workspace or library root holds it
            return;
        }
        // If the opened editor was in the orphan root, the file is now stale
        // and unreachable. Move it to the stale root.

        let mut orphan_files = orphan.files(self).clone();
        orphan_files.retain(|f| *f != file);
        orphan.set_files(self).to(orphan_files);

        let stale = self.stale_root();
        let mut stale_files = stale.files(self).clone();
        stale_files.push(file);
        stale.set_files(self).to(stale_files);
    }

    fn add_watched_file(&mut self, url: UrlId, contents: String) {
        watch::add_watched_file(self, url, contents);
    }

    fn remove_watched_file(&mut self, url: UrlId) {
        watch::remove_watched_file(self, url);
    }

    fn apply_watcher_events(&mut self, events: Vec<FileEvent>, editor_owned: &HashSet<UrlId>) {
        watch::apply_watcher_events(self, events, editor_owned);
    }
}

/// Extension methods on [`Root`] for placement-aware updates.
///
/// These are the public surface for scanners and the LSP to push their
/// findings into the salsa input graph. Implementations live in
/// `oak_scan` because they coordinate across multiple input fields
/// (`Root.scripts`, `Package.files`, `File.package`) in ways the raw
/// salsa setters can't express on their own.
pub trait RootExt {
    /// Create or update a package under this root. Atomic
    /// full-replacement of the package's file set.
    ///
    /// Identity is keyed on `description_url`: if `self.packages`
    /// already contains a `Package` with this URL, that entity is
    /// reused and its `name` / `version` / `namespace` / `collation`
    /// fields are updated in place. Salsa backdates each setter call
    /// when the value doesn't actually change.
    ///
    /// Files are reused by URL via [`Db::file_by_url`]; see
    /// [`FileEntry`] for the content-preservation semantics. Wiring
    /// the returned `Package` into `self.packages` is the caller's
    /// job.
    fn set_package<DB: Db + DbInputs>(
        self,
        db: &mut DB,
        description_url: UrlId,
        name: String,
        version: Option<String>,
        namespace: Namespace,
        files: Vec<FileEntry>,
        scripts: Vec<FileEntry>,
        collation: Option<Vec<String>>,
    ) -> Package;

    /// Drop this root from its live container, rehoming its files and packages
    /// so they survive the eviction for later reuse.
    ///
    /// `editor_owned` is `None` for callers without an editor concept (the
    /// library scanner) and `Some(&set)` for the workspace scanner. Files in
    /// `editor_owned` go to `OrphanRoot` (still analysis-visible since the
    /// buffer is open). Everything else goes to `StaleRoot`
    /// (analysis-invisible, available for entity reuse on the next
    /// `set_*_paths` call). `Package` entities always go to stale.
    ///
    /// Doesn't touch `LibraryRoots` / `WorkspaceRoots`. The caller is
    /// responsible for rebuilding those Vec inputs with `self` excluded.
    fn set_stale<DB: Db + DbInputs>(self, db: &mut DB, editor_owned: Option<&HashSet<UrlId>>);

    /// Replace `self.scripts` with `File` entities for `files`. Same identity
    /// rules as [`set_package`](Self::set_package): existing `File` entities at
    /// the given URLs are reused and have their `package` field cleared.
    fn set_workspace_scripts<DB: Db + DbInputs>(self, db: &mut DB, files: Vec<FileEntry>);
}

impl RootExt for Root {
    fn set_package<DB: Db + DbInputs>(
        self,
        db: &mut DB,
        description_url: UrlId,
        name: String,
        version: Option<String>,
        namespace: Namespace,
        files: Vec<FileEntry>,
        scripts: Vec<FileEntry>,
        collation: Option<Vec<String>>,
    ) -> Package {
        // `package_by_url()` finds the existing entity whether it's already
        // in `self.packages` (rescan, common path) or in
        // `stale_root.packages` (resurrection after a previous eviction).
        // Either way we reuse the entity and refresh its metadata fields.
        let pkg = match package_by_url(db, &description_url) {
            Some(p) => {
                p.set_name(db).to(name);
                p.set_version(db).to(version);
                p.set_namespace(db).to(namespace);
                p.set_collation(db).to(collation);
                remove_from_stale_packages(db, p);
                p
            },
            None => Package::new(
                db,
                description_url,
                name,
                version,
                namespace,
                Vec::new(),
                Vec::new(),
                collation,
            ),
        };

        let file_entities: Vec<File> = files
            .into_iter()
            .map(|entry| upsert_root_file(db, Some(pkg), entry))
            .collect();
        let script_entities: Vec<File> = scripts
            .into_iter()
            .map(|entry| upsert_root_file(db, Some(pkg), entry))
            .collect();

        pkg.set_files(db).to(file_entities);
        pkg.set_scripts(db).to(script_entities);
        pkg
    }

    fn set_stale<DB: Db + DbInputs>(self, db: &mut DB, editor_owned: Option<&HashSet<UrlId>>) {
        crate::stale::set_root_stale(db, self, editor_owned);
    }

    fn set_workspace_scripts<DB: Db + DbInputs>(self, db: &mut DB, files: Vec<FileEntry>) {
        let scripts: Vec<File> = files
            .into_iter()
            .map(|entry| upsert_root_file(db, None, entry))
            .collect();
        self.set_scripts(db).to(scripts);
    }
}

/// Upsert a `File` for `entry`, set its `package` backpointer, and clean up
/// stale references in old containers.
///
/// **Caller invariant.** The caller must atomically place the returned `File`
/// in some `Root` container (`pkg.files` or `root.scripts`) before returning.
/// Three callers:
///
/// - [`RootExt::set_package`] (both library and workspace scanners)
/// - [`RootExt::set_workspace_scripts`] (workspace scanner)
/// - [`watch::add_watched_file`] (watcher dispatch)
///
/// The orphan cleanup below relies on this contract. A future caller that
/// invoked `upsert_root_file()` without then placing the file would leave it
/// with no container, and `file_by_url()` would return `None`.
pub(crate) fn upsert_root_file<DB: Db + DbInputs>(
    db: &mut DB,
    package: Option<Package>,
    entry: FileEntry,
) -> File {
    if let Some(existing) = db.file_by_url(&entry.url) {
        // The new container is owned by the caller. What needs active cleanup
        // is the OLD container:
        //
        // - If the package backpointer changed and the old package was Some,
        //   that package's `files` vec still references this file. Drop it,
        //   otherwise the old `Package` would carry a stale entry until its
        //   next wholesale rescan.
        //
        // - If the file was in `OrphanRoot.files` (e.g. the editor had it open
        //   before a scan classified it), drop it. Per the caller invariant
        //   the file is about to land in a `Root` container, so the orphan
        //   reference is stale by the time this returns.
        let old_package = existing.package(db);
        existing.set_package(db).to(package);
        if old_package != package {
            if let Some(old_pkg) = old_package {
                remove_from_pkg_files(db, old_pkg, existing);
            }
        }
        remove_from_orphan(db, existing);
        return existing;
    }

    if let Some(stale) = stale_file_by_url(db, &entry.url) {
        // Resurrecting an evicted File. Restore disk contents (the editor-owned
        // variant lives in `orphan_root` instead; this branch only sees
        // scanner-discovered files).
        stale.set_contents(db).to(entry.contents);
        stale.set_package(db).to(package);
        remove_from_stale_files(db, stale);
        return stale;
    }

    File::new(db, entry.url, entry.contents, package)
}

/// Remove `file` from whichever of `pkg.files` / `pkg.scripts` holds it.
/// Used during cross-package moves: if a file's owning package changed,
/// the old package's containers still reference it until we drop the
/// entry. Also used by [`watch::remove_watched_file`] when a file
/// disappears from disk.
pub(crate) fn remove_from_pkg_files<DB: Db + DbInputs>(db: &mut DB, pkg: Package, file: File) {
    if pkg.files(db).contains(&file) {
        let mut files = pkg.files(db).clone();
        files.retain(|f| *f != file);
        pkg.set_files(db).to(files);
        return;
    }

    if pkg.scripts(db).contains(&file) {
        let mut scripts = pkg.scripts(db).clone();
        scripts.retain(|f| *f != file);
        pkg.set_scripts(db).to(scripts);
    }
}

fn remove_from_orphan<DB: Db + DbInputs>(db: &mut DB, file: File) {
    let orphan = db.orphan_root();
    if !orphan.files(db).contains(&file) {
        return;
    }
    let mut files = orphan.files(db).clone();
    files.retain(|f| *f != file);
    orphan.set_files(db).to(files);
}

fn add_to_orphan_files<DB: Db + DbInputs>(db: &mut DB, file: File) {
    let orphan = db.orphan_root();
    let mut files = orphan.files(db).clone();
    if !files.contains(&file) {
        files.push(file);
        orphan.set_files(db).to(files);
    }
}
