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
use std::hash::Hash;
use std::path::Path;
use std::path::PathBuf;

use aether_path::FilePath;
use oak_db::Db;
use oak_db::DbInputs;
use oak_db::File;
use oak_db::FileRevision;
use oak_db::Package;
use oak_db::Root;
use salsa::Setter;

use crate::lookup::package_by_path;
use crate::packages::read_package_sources;
use crate::stale::remove_from_stale_files;
use crate::stale::remove_from_stale_packages;
use crate::stale::stale_file_by_path;

/// Description of one R file the scanner wants to register.
///
/// `revision` is the file's mtime read during the walk. It drives cache
/// invalidation for the lazy `source_text` query, so a rescan that finds a
/// newer mtime forces the next `source_text` to re-read from disk.
///
/// If a `File` already exists at this URL (scanner-created from an
/// earlier scan, or VFS-created via `didOpen`), the helpers reuse that
/// entity. The editor override set by `upsert_editor` is authoritative
/// for open buffers.
#[derive(Clone, Debug)]
pub struct FileEntry {
    pub path: FilePath,
    pub revision: FileRevision,
}

/// Extension methods on the database for scanner orchestration and
/// placement-aware updates that don't have a natural `Root` receiver.
///
/// Workspace-level orchestration (path diff, watcher dispatch, rescan
/// coalescing) lives on [`crate::ScanScheduler`] instead, since it
/// needs scheduler state that can't be kept in salsa inputs.
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
    ///
    /// **Why this is sync while workspaces go through
    /// [`crate::ScanScheduler`].** Libraries are scanned exactly
    /// once at LSP init today. Workspaces churn (folders open and
    /// close at any time) and have a file watcher pushing events
    /// mid-scan, so the workspace path needs the buffering /
    /// stale-result machinery the scheduler exists for. Libraries
    /// have neither, so the extra plumbing buys nothing. If
    /// `.libPaths()` ever becomes mutable mid-session (e.g. user
    /// runs `.libPaths(...)` in the console), this should join
    /// the scheduler.
    fn set_library_paths(&mut self, paths: &[PathBuf]);

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
    fn upsert_editor(&mut self, path: FilePath, contents: String) -> File;

    /// Mark the editor as no longer holding a buffer for this URL.
    ///
    /// If the file lives in [`OrphanRoot`] (placed there by
    /// [`Self::upsert_editor`] because the URL didn't belong to a live root, or
    /// by workspace eviction routing for an open buffer in a removed
    /// workspace), it gets moved to [`StaleRoot`]. Future
    /// [`Self::upsert_editor`] for the same URL resurrects the entity from
    /// stale instead of minting a fresh one.
    ///
    /// If the file is in a live workspace / library container, the call is a
    /// no-op.
    fn close_editor(&mut self, path: &FilePath);

    /// Set `package`'s `files` / `scripts` to the `.R` files found directly
    /// under `directory`, respecting the package's `Collate` rules.
    ///
    /// Used to ingest sources produced by an external tool into an already-registered
    /// library `Package` whose `files` start empty (the scanner registers installed
    /// packages without sources).
    fn set_package_sources(&mut self, package: Package, directory: &Path);
}

impl<DB: Db + DbInputs> DbScan for DB {
    fn set_library_paths(&mut self, paths: &[PathBuf]) {
        crate::library::set_library_paths(self, paths);
    }

    fn upsert_editor(&mut self, path: FilePath, contents: String) -> File {
        if let Some(existing) = self.file_by_path(&path) {
            existing.set_source_text_override(self).to(Some(contents));
            return existing;
        }

        // Resurrect a previously-closed buffer from stale. The didOpen
        // content overwrites whatever the stale entity carried.
        if let Some(stale) = stale_file_by_path(self, &path) {
            stale.set_source_text_override(self).to(Some(contents));
            stale.set_package(self).to(None);
            remove_from_stale_files(self, stale);
            add_to_orphan_files(self, stale);
            return stale;
        }

        let file = File::new(self, path, FileRevision::zero(), Some(contents), None);
        add_to_orphan_files(self, file);
        file
    }

    fn close_editor(&mut self, path: &FilePath) {
        let Some(file) = self.file_by_path(path) else {
            return;
        };

        // Clear the editor override so the disk contents becomes the source of
        // truth again. This immediately invalidates queries that depend on
        // `source_text`, because the latter depends on the override.
        file.set_source_text_override(self).to(None);

        let orphan = self.orphan_root();
        let Some(orphan_files) = with_cow_remove(orphan.files(self), file) else {
            // A workspace or library root holds it, nothing to do.
            return;
        };
        // The opened editor was in the orphan root, so the file is now stale
        // and unreachable. Move it to the stale root.
        orphan.set_files(self).to(orphan_files);

        let stale = self.stale_root();
        if let Some(stale_files) = with_cow_insert(stale.files(self), file) {
            stale.set_files(self).to(stale_files);
        }
    }

    fn set_package_sources(&mut self, package: Package, directory: &Path) {
        let (files, scripts) = read_package_sources(directory, package.collation(self).as_deref());

        let files: Vec<File> = files
            .into_iter()
            .map(|entry| upsert_root_file(self, Some(package), entry))
            .collect();

        let scripts: Vec<File> = scripts
            .into_iter()
            .map(|entry| upsert_root_file(self, Some(package), entry))
            .collect();

        if package.files(self) != &files {
            package.set_files(self).to(files);
        }
        if package.scripts(self) != &scripts {
            package.set_scripts(self).to(scripts);
        }
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
    /// Create or update a package under this root. Atomic full-replacement of
    /// the package's file set.
    ///
    /// The package identity is keyed on `description_path`: if `self.packages`
    /// already contains a `Package` with this URL, that entity is reused and
    /// its `name` / `description_revision` / `namespace_revision` fields are
    /// refreshed (only where they actually changed to avoid an unnecessary
    /// revision bump).
    ///
    /// Invariant: The caller must wire the returned `Package` into
    /// `self.packages` via `Root::set_packages()`.
    fn set_package<DB: Db + DbInputs>(
        self,
        db: &mut DB,
        description_path: FilePath,
        name: String,
        description_revision: FileRevision,
        namespace_revision: FileRevision,
        index_revision: Option<FileRevision>,
        files: Vec<FileEntry>,
        scripts: Vec<FileEntry>,
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
    fn set_stale<DB: Db + DbInputs>(self, db: &mut DB, editor_owned: Option<&HashSet<FilePath>>);

    /// Replace `self.scripts` with `File` entities for `files`. Same identity
    /// rules as [`set_package`](Self::set_package): existing `File` entities at
    /// the given URLs are reused and have their `package` field cleared.
    fn set_workspace_scripts<DB: Db + DbInputs>(self, db: &mut DB, files: Vec<FileEntry>);
}

impl RootExt for Root {
    fn set_package<DB: Db + DbInputs>(
        self,
        db: &mut DB,
        description_path: FilePath,
        name: String,
        description_revision: FileRevision,
        namespace_revision: FileRevision,
        index_revision: Option<FileRevision>,
        files: Vec<FileEntry>,
        scripts: Vec<FileEntry>,
    ) -> Package {
        // `package_by_path()` finds the existing entity whether it's already
        // in `self.packages` (rescan, common path) or in
        // `stale_root.packages` (resurrection after a previous eviction).
        // Either way we reuse the entity and refresh its metadata fields.
        let pkg = match package_by_path(db, &description_path) {
            Some(p) => {
                if p.name(db) != &name {
                    p.set_name(db).to(name);
                }
                if p.description_revision(db) != description_revision {
                    p.set_description_revision(db).to(description_revision);
                }
                if p.namespace_revision(db) != namespace_revision {
                    p.set_namespace_revision(db).to(namespace_revision);
                }
                remove_from_stale_packages(db, p);
                p
            },
            None => Package::new(
                db,
                description_path,
                name,
                description_revision,
                namespace_revision,
                index_revision,
                None,
                Vec::new(),
                Vec::new(),
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

        if pkg.files(db) != &file_entities {
            pkg.set_files(db).to(file_entities);
        }
        if pkg.scripts(db) != &script_entities {
            pkg.set_scripts(db).to(script_entities);
        }
        pkg
    }

    fn set_stale<DB: Db + DbInputs>(self, db: &mut DB, editor_owned: Option<&HashSet<FilePath>>) {
        crate::stale::set_root_stale(db, self, editor_owned);
    }

    fn set_workspace_scripts<DB: Db + DbInputs>(self, db: &mut DB, files: Vec<FileEntry>) {
        let scripts: Vec<File> = files
            .into_iter()
            .map(|entry| upsert_root_file(db, None, entry))
            .collect();
        if self.scripts(db) != &scripts {
            self.set_scripts(db).to(scripts);
        }
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
/// with no container, and `file_by_path()` would return `None`.
pub(crate) fn upsert_root_file<DB: Db + DbInputs>(
    db: &mut DB,
    package: Option<Package>,
    entry: FileEntry,
) -> File {
    if let Some(existing) = db.file_by_path(&entry.path) {
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
        if old_package != package {
            existing.set_package(db).to(package);
            if let Some(old_pkg) = old_package {
                remove_from_pkg_files(db, old_pkg, existing);
            }
        }
        remove_from_orphan(db, existing);
        return existing;
    }

    if let Some(stale) = stale_file_by_path(db, &entry.path) {
        // Resurrecting an evicted File. A resurrected scanner file is
        // disk-backed, so clear any stale override and update the revision.
        // The editor-owned variant lives in `orphan_root` instead; this branch
        // only sees scanner-discovered files.
        if stale.revision(db) != entry.revision {
            stale.set_revision(db).to(entry.revision);
        }
        if stale.source_text_override(db).is_some() {
            stale.set_source_text_override(db).to(None);
        }
        if stale.package(db) != package {
            stale.set_package(db).to(package);
        }
        remove_from_stale_files(db, stale);
        return stale;
    }

    File::new(db, entry.path, entry.revision, None, package)
}

/// Remove `file` from whichever of `pkg.files` / `pkg.scripts` holds it.
/// Used during cross-package moves: if a file's owning package changed,
/// the old package's containers still reference it until we drop the
/// entry. Also used by [`watch::remove_watched_file`] when a file
/// disappears from disk.
pub(crate) fn remove_from_pkg_files<DB: Db + DbInputs>(db: &mut DB, pkg: Package, file: File) {
    if let Some(files) = with_cow_filter(pkg.files(db), file) {
        pkg.set_files(db).to(files);
        return;
    }
    if let Some(scripts) = with_cow_filter(pkg.scripts(db), file) {
        pkg.set_scripts(db).to(scripts);
    }
}

pub(crate) fn remove_from_orphan<DB: Db + DbInputs>(db: &mut DB, file: File) {
    let orphan = db.orphan_root();
    if let Some(files) = with_cow_remove(orphan.files(db), file) {
        orphan.set_files(db).to(files);
    }
}

fn add_to_orphan_files<DB: Db + DbInputs>(db: &mut DB, file: File) {
    let orphan = db.orphan_root();
    if let Some(files) = with_cow_insert(orphan.files(db), file) {
        orphan.set_files(db).to(files);
    }
}

/// The ordered container with `file` appended, or `None` if it's already there.
///
/// `None` means nothing would change, so the caller skips the salsa write and
/// the clone. This keeps the "clone only when the field actually changes" rule
/// in one place, shared by the ordered container updates on `Root` and
/// `Package`. See [`with_inserted`] / [`with_discarded`] for the unordered
/// `OrphanRoot` / `StaleRoot` sets.
pub(crate) fn with_cow_push<T: Clone + PartialEq>(files: &[T], file: T) -> Option<Vec<T>> {
    if files.contains(&file) {
        return None;
    }
    let mut updated = files.to_vec();
    updated.push(file);
    Some(updated)
}

/// The ordered container with `file` removed, or `None` if it wasn't there.
/// `None` means nothing would change, see [`with_appended`].
pub(crate) fn with_cow_filter<T: Clone + PartialEq>(files: &[T], file: T) -> Option<Vec<T>> {
    if !files.contains(&file) {
        return None;
    }
    Some(files.iter().filter(|f| **f != file).cloned().collect())
}

/// The set with `item` inserted, or `None` if it's already present. The
/// unordered counterpart of [`with_appended`], used for the `OrphanRoot` /
/// `StaleRoot` sets where membership is all that matters.
pub(crate) fn with_cow_insert<T: Clone + Eq + Hash>(
    set: &HashSet<T>,
    item: T,
) -> Option<HashSet<T>> {
    if set.contains(&item) {
        return None;
    }
    let mut updated = set.clone();
    updated.insert(item);
    Some(updated)
}

/// The set with `item` removed, or `None` if it wasn't present. The unordered
/// counterpart of [`with_removed`].
pub(crate) fn with_cow_remove<T: Clone + Eq + Hash>(
    set: &HashSet<T>,
    item: T,
) -> Option<HashSet<T>> {
    if !set.contains(&item) {
        return None;
    }
    let mut updated = set.clone();
    updated.remove(&item);
    Some(updated)
}
