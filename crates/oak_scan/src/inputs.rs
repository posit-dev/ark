//! Update helpers for `oak_db`. Each helper drives one update pattern
//! that a single salsa setter can't safely express on its own.
//!
//! [`oak_db::IdentityMap`] preserves `File` identity by path and `Package`
//! identity by `DESCRIPTION` path, independently of container membership.
//! Roots are reused by path while they remain live. Rescans update existing
//! entities in place, preserving downstream Salsa caches for files whose
//! content is unchanged.
//!
//! The trade-off is a small placement invariant: `file.package` must
//! agree with which container Vec holds the file (`pkg.files`,
//! `root.scripts`, or `orphan_root().files`). Outside callers should
//! not call `file.set_package(...)` directly. This crate is the only
//! intended caller of the placement-affecting setters on `oak_db`'s
//! input structs.

use std::collections::HashSet;
use std::hash::Hash;
use std::mem;
use std::path::Path;
use std::path::PathBuf;
use std::thread;

use aether_path::FilePath;
use oak_db::Db;
use oak_db::DbInputs;
use oak_db::File;
use oak_db::FileRevision;
use oak_db::Notebook;
use oak_db::Package;
use oak_db::Root;
use salsa::Setter;

use crate::packages::read_package_sources;

/// Description of one R file the scanner wants to register.
///
/// `revision` is the file's mtime read during the walk. It drives cache
/// invalidation for the lazy `source_text` query, so a rescan that finds a
/// newer mtime forces the next `source_text` to re-read from disk.
///
/// If a `File` already exists at this path (scanner-created from an
/// earlier scan, or created via `didOpen`), the helpers reuse that
/// entity. The editor override set by `upsert_editor()` is authoritative
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
    ///   salsa churn. Watcher-driven rescans are handled separately by
    ///   [`crate::ScanScheduler::apply_watcher_events()`].
    ///
    /// - New paths are scanned synchronously and added.
    ///
    /// - Removed paths have their roots evicted.
    ///   Open buffers no longer listed by a live root stay visible through
    ///   [`OrphanRoot`](oak_db::OrphanRoot).
    ///   Re-adding a path reuses its `File` and `Package` entities through
    ///   [`oak_db::IdentityMap`].
    ///
    /// Order in `LibraryRoots.roots` follows `paths`, matching R's
    /// `.libPaths()` precedence.
    ///
    /// Synchronous scans make installed packages available to the first LSP
    /// diagnostics at startup. If this call removes a root with a rescan in
    /// flight, [`crate::ScanScheduler::apply_scan_completed()`] drops its result
    /// because the root is no longer in `library_roots`.
    fn set_library_paths(&mut self, paths: &[PathBuf]);

    /// Upsert the editor's view of a file. Used by the LSP layer to apply
    /// `didOpen` / `didChange` content for any URL the editor touches.
    ///
    /// Every call for the same path returns the same `File` through
    /// [`oak_db::IdentityMap`]. The supplied contents replace its editor
    /// override and register it as an open buffer. The buffer remains visible
    /// to [`file_by_path()`](oak_db::SourceDb::file_by_path) across scans,
    /// watcher events, source replacements, and root eviction until
    /// [`Self::close_editor()`].
    ///
    /// A file visible through a live root or `OrphanRoot` keeps its
    /// classification. Opening a buffer is a content event, not a
    /// reclassification.
    ///
    /// A file outside all live containers is added to `OrphanRoot.files` with
    /// no package so analysis can see the buffer. A scan or watcher event can
    /// later place it in a root or package.
    fn upsert_editor(&mut self, path: FilePath, contents: String) -> File;

    /// Release the editor buffer for this path.
    ///
    /// Clears the override and unregisters the open buffer even if the file
    /// is outside all live containers, making disk contents authoritative.
    ///
    /// Removes any membership in [`OrphanRoot`](oak_db::OrphanRoot), whether
    /// opening the buffer or a later write placed it there. The file becomes
    /// invisible to analysis if no live root lists it. Membership in live
    /// workspace and library containers is unchanged, and a later
    /// [`Self::upsert_editor()`] reuses the same entity.
    ///
    /// An unknown path is a no-op and creates no entity.
    fn close_editor(&mut self, path: &FilePath);

    /// Record the notebook at `path` with its R code cells in document order.
    /// Each cell must already be an editor buffer from [`Self::upsert_editor()`].
    ///
    /// Reuses the [`Notebook`] entity when `path` is already open, so a
    /// structural edit only updates the cell list.
    fn set_notebook_cells(&mut self, path: FilePath, cells: Vec<File>) -> Notebook;

    /// Removes notebook-based cross-cell visibility without closing the cell
    /// buffers. Close those separately with [`Self::close_editor()`].
    fn close_notebook(&mut self, path: &FilePath);

    /// Replace source files and scripts with the `.R` files directly under
    /// `directory`, applying `package`'s `Collate:` rules. Preserve generated
    /// files after the source files.
    ///
    /// Used to ingest sources produced by an external tool into an already-registered
    /// library `Package` whose `files` start empty (the scanner registers installed
    /// packages without sources).
    ///
    /// Open buffers absent from the replacement sources stay visible through
    /// [`OrphanRoot`](oak_db::OrphanRoot) if no other live container lists them.
    fn set_package_sources(&mut self, package: Package, directory: &Path);

    /// Store `package`'s host-supplied facts for its current `DESCRIPTION`
    /// revision, and record that revision as [`Package::facts_revision()`].
    ///
    /// The caller must supply facts for the current revision. Generated files
    /// are replaced with the `.R` files directly under `generated` (such as
    /// `base`'s primitive stubs), or cleared when it is `None`. In
    /// [`Package::files()`], they follow all source files and are ordered by
    /// basename. `Collate:` does not apply to generated files. Lazy data names
    /// are replaced with `lazy_data`. Source files and [`Package::scripts()`]
    /// are preserved, so either ingestion job can finish first.
    ///
    /// Open buffers among the dropped generated files stay visible through
    /// [`OrphanRoot`](oak_db::OrphanRoot) if no other live container lists them.
    fn set_package_facts(
        &mut self,
        package: Package,
        generated: Option<&Path>,
        lazy_data: Vec<String>,
    );
}

impl<DB: Db + DbInputs> DbScan for DB {
    fn set_library_paths(&mut self, paths: &[PathBuf]) {
        crate::library::set_library_paths(self, paths);
        keep_open_files_visible(self);
    }

    fn upsert_editor(&mut self, path: FilePath, contents: String) -> File {
        let file = self.intern_file(path, FileRevision::zero(), None);
        let visible = self.file_by_path(file.path(self)).is_some();

        self.set_editor_buffer(file, contents);

        if !visible {
            // Files in `OrphanRoot` must have no package. This file can retain
            // a backpointer to a package omitted from all roots by a rescan,
            // because the unpublished package still lists its files.
            if file.package(self).is_some() {
                file.set_package(self).to(None);
            }
            add_to_orphan_files(self, file);
        }
        file
    }

    fn close_editor(&mut self, path: &FilePath) {
        let Some(file) = self.interned_file(path) else {
            return;
        };

        self.clear_editor_buffer(file);

        // A file in a workspace or library container keeps its placement.
        remove_from_orphan(self, file);
    }

    fn set_notebook_cells(&mut self, path: FilePath, cells: Vec<File>) -> Notebook {
        let open = self.open_notebooks();
        let existing = open
            .notebooks(self)
            .iter()
            .copied()
            .find(|notebook| notebook.path(self) == &path);

        if let Some(notebook) = existing {
            if notebook.cells(self) != &cells {
                notebook.set_cells(self).to(cells);
            }
            return notebook;
        }

        let notebook = Notebook::new(self, path, cells);
        let mut notebooks = open.notebooks(self).clone();
        notebooks.push(notebook);
        open.set_notebooks(self).to(notebooks);
        notebook
    }

    fn close_notebook(&mut self, path: &FilePath) {
        let open = self.open_notebooks();
        let notebooks: Vec<Notebook> = open
            .notebooks(self)
            .iter()
            .copied()
            .filter(|notebook| notebook.path(self) != path)
            .collect();

        if notebooks.len() != open.notebooks(self).len() {
            open.set_notebooks(self).to(notebooks);
        }
    }

    fn set_package_sources(&mut self, package: Package, directory: &Path) {
        let (files, scripts) = read_package_sources(directory, package.collation(self).as_deref());
        let mut upserts = FileUpserts::new();
        set_package_source_entries(self, &mut upserts, package, files, scripts);

        // The LSP drops completions fetched for an older `DESCRIPTION`
        // revision (`SourceScheduler::finish()`), so these sources are for the
        // current one.
        let revision = Some(package.description_revision(self));
        if package.sources_revision(self) != revision {
            package.set_sources_revision(self).to(revision);
        }

        upserts.finish(self);
        keep_open_files_visible(self);
    }

    fn set_package_facts(
        &mut self,
        package: Package,
        generated: Option<&Path>,
        lazy_data: Vec<String>,
    ) {
        let previous = package.files(self).to_vec();
        match generated {
            Some(directory) => set_generated_files_from(self, package, directory),
            None => replace_package_membership(self, package, |db| {
                package.set_generated_files(db, Vec::new())
            }),
        }
        package.set_lazy_data(self, lazy_data);

        // The LSP drops facts computed for an older `DESCRIPTION` revision
        // (`SidecarScheduler::finish()`), so these facts are for the current one.
        let revision = Some(package.description_revision(self));
        if package.facts_revision(self) != revision {
            package.set_facts_revision(self).to(revision);
        }

        // If membership is unchanged, no open buffer lost visibility. Avoid
        // revalidating `file_by_path()` for every open buffer after writing
        // `Package::facts_revision()` in that case.
        if package.files(self) != previous.as_slice() {
            keep_open_files_visible(self);
        }
    }
}

fn set_generated_files_from<DB: Db + DbInputs>(db: &mut DB, package: Package, directory: &Path) {
    // Generated bindings must be indexed even though `Collate:` omits them.
    let (generated, _) = read_package_sources(directory, None);
    let mut upserts = FileUpserts::new();

    let generated: Vec<File> = generated
        .into_iter()
        .map(|entry| upserts.upsert(db, Some(package), entry))
        .collect();

    replace_package_membership(db, package, |db| package.set_generated_files(db, generated));
    upserts.finish(db);
}

fn set_package_source_entries<DB: Db + DbInputs>(
    db: &mut DB,
    upserts: &mut FileUpserts,
    package: Package,
    files: Vec<FileEntry>,
    scripts: Vec<FileEntry>,
) {
    let files: Vec<File> = files
        .into_iter()
        .map(|entry| upserts.upsert(db, Some(package), entry))
        .collect();

    let scripts: Vec<File> = scripts
        .into_iter()
        .map(|entry| upserts.upsert(db, Some(package), entry))
        .collect();

    set_package_containers(db, package, files, scripts);
}

/// Keep open editor buffers reachable through
/// [`OrphanRoot`](oak_db::OrphanRoot) when a write removes them from every live
/// container, as happens when a scan no longer finds their files on disk or
/// their root is evicted. Otherwise `file_by_path()` would miss an entity still
/// held by the editor and analysis would lose the buffer.
///
/// Public entry points that can remove files from live containers call this
/// after all placement updates and buffered watcher events. Internal helpers
/// may leave open buffers temporarily invisible.
///
/// Files still reachable retain their container and package, including
/// entities shared with another root. Calling `set_package()` on a shared
/// `Package` can remove files from another root's view, so every open buffer
/// is checked, not just those under the root a write touched.
///
/// Every lookup runs before the first write, so the orphan path index isn't
/// invalidated and rebuilt once per rescued file.
pub(crate) fn keep_open_files_visible<DB: Db + DbInputs>(db: &mut DB) {
    let mut dropped: Vec<File> = Vec::new();
    for file in db.editor_buffer_files() {
        let path = file.path(db);
        match db.file_by_path(path) {
            None => dropped.push(file),
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
    orphan_files.extend(dropped);
    orphan.set_files(db).to(orphan_files);
}

/// Extension methods on [`Root`] for placement-aware updates.
///
/// These methods keep container membership and package backpointers consistent.
/// They are crate-private because they may leave open buffers invisible until
/// the public entry point calls [`keep_open_files_visible()`].
pub(crate) trait RootExt {
    /// Create or update a package under this root. Atomic full-replacement of
    /// the package's source files and scripts. Generated files stay after the
    /// source files.
    ///
    /// [`oak_db::IdentityMap`] gives each `description_path` one `Package`
    /// for the database's lifetime, regardless of root membership. Reusing
    /// the entity refreshes its metadata, writing only changed fields to
    /// avoid unnecessary revision bumps.
    ///
    /// Invariant: The caller must wire the returned `Package` into
    /// `self.packages` via `Root::set_packages()`.
    ///
    /// Scanners batch their file upserts through [`upsert_package()`] or keep
    /// fetched sources through [`upsert_package_metadata()`]. Tests use this
    /// to build packages under hand-made roots.
    #[cfg(test)]
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

    /// Empty this root's containers and its packages' containers, and clear
    /// the backpointers of the files they listed. Entities stay in
    /// [`oak_db::IdentityMap`], so re-adding the root later reuses them.
    ///
    /// The files become analysis-invisible unless a surviving nested root
    /// still lists them. The caller keeps open buffers visible with
    /// [`keep_open_files_visible()`].
    ///
    /// Doesn't touch `LibraryRoots` / `WorkspaceRoots`. The caller is
    /// responsible for rebuilding those Vec inputs with `self` excluded.
    fn evict<DB: Db + DbInputs>(self, db: &mut DB);
}

impl RootExt for Root {
    #[cfg(test)]
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
        let mut upserts = FileUpserts::new();
        let pkg = upsert_package(
            db,
            &mut upserts,
            description_path,
            name,
            description_revision,
            namespace_revision,
            index_revision,
            files,
            scripts,
        );
        upserts.finish(db);
        pkg
    }

    fn evict<DB: Db + DbInputs>(self, db: &mut DB) {
        crate::eviction::evict_root(db, self);
    }
}

/// Create or update a package and replace its file set, with files resolved
/// through `upserts`, which the caller finishes once every container is
/// written. Metadata is refreshed as in [`upsert_package_metadata()`].
pub(crate) fn upsert_package<DB: Db + DbInputs>(
    db: &mut DB,
    upserts: &mut FileUpserts,
    description_path: FilePath,
    name: String,
    description_revision: FileRevision,
    namespace_revision: FileRevision,
    index_revision: Option<FileRevision>,
    files: Vec<FileEntry>,
    scripts: Vec<FileEntry>,
) -> Package {
    let pkg = upsert_package_metadata(
        db,
        description_path,
        name,
        description_revision,
        namespace_revision,
        index_revision,
    );

    set_package_source_entries(db, upserts, pkg, files, scripts);

    // Workspace files are not fetched sources. A `Package` that was installed
    // before becoming a source checkout must not keep its fetched revision,
    // or a later reinstall with the same `DESCRIPTION` revision would keep
    // these lists as its sources.
    if pkg.sources_revision(db).is_some() {
        pkg.set_sources_revision(db).to(None);
    }
    pkg
}

/// Intern the [`Package`] at `description_path` and refresh its metadata,
/// leaving its `files` and `scripts` untouched so library rescans keep fetched
/// sources. Writes only changed fields to avoid bumping the Salsa revision on
/// unchanged rescans.
///
/// The caller must place the result in a root's `packages` via
/// [`Root::set_packages()`] for analysis to see it.
pub(crate) fn upsert_package_metadata<DB: Db + DbInputs>(
    db: &mut DB,
    description_path: FilePath,
    name: String,
    description_revision: FileRevision,
    namespace_revision: FileRevision,
    index_revision: Option<FileRevision>,
) -> Package {
    let pkg = db.intern_package(
        description_path,
        name.clone(),
        description_revision,
        namespace_revision,
        index_revision,
    );
    if pkg.name(db) != &name {
        pkg.set_name(db).to(name);
    }
    if pkg.description_revision(db) != description_revision {
        pkg.set_description_revision(db).to(description_revision);
    }
    if pkg.namespace_revision(db) != namespace_revision {
        pkg.set_namespace_revision(db).to(namespace_revision);
    }
    if pkg.index_revision(db) != index_revision {
        pkg.set_index_revision(db).to(index_revision);
    }
    pkg
}

/// Replace `pkg`'s source files and `pkg.scripts()`, keeping its generated
/// files, through [`replace_package_membership()`].
fn set_package_containers<DB: Db + DbInputs>(
    db: &mut DB,
    pkg: Package,
    files: Vec<File>,
    scripts: Vec<File>,
) {
    replace_package_membership(db, pkg, |db| {
        pkg.set_source_files(db, files);
        if pkg.scripts(db) != &scripts {
            pkg.set_scripts(db).to(scripts);
        }
    });
}

/// Run `write` on `pkg`'s membership, then keep package backpointers
/// consistent with it. `write` skips unchanged fields.
///
/// Files that `write` drops from both `pkg.files()` and `pkg.scripts()` lose
/// their backpointer to `pkg` if it still points there.
/// [`oak_db::IdentityMap`] retains these files even when they are invisible to
/// analysis, so a later writer can reuse them without inheriting a backpointer
/// to a package that does not list them.
pub(crate) fn replace_package_membership<DB: Db + DbInputs>(
    db: &mut DB,
    pkg: Package,
    write: impl FnOnce(&mut DB),
) {
    let previous: Vec<File> = pkg
        .files(db)
        .iter()
        .chain(pkg.scripts(db))
        .copied()
        .collect();
    write(db);

    let listed: HashSet<File> = pkg
        .files(db)
        .iter()
        .chain(pkg.scripts(db))
        .copied()
        .collect();
    for file in previous {
        // Preserve the destination backpointer if an earlier upsert in the
        // batch moved this file to another package.
        if !listed.contains(&file) && file.package(db) == Some(pkg) {
            file.set_package(db).to(None);
        }
    }
}

/// Replace `root.scripts` using the batch
/// [`upserts`](FileUpserts). The caller must call [`FileUpserts::finish()`]
/// after writing all containers to remove the files' orphan membership.
///
/// A script that another live workspace root lists in a package keeps that
/// package, both as backpointer and as membership. With nested roots `pkg` and
/// `pkg/R`, the inner root can't see `DESCRIPTION` and lists `R/a.R` as a
/// script. Without this rule, whichever scan applies last would decide whether
/// `pkg` lists the file, and pool completion order isn't fixed. The watcher
/// places new files the same way.
pub(crate) fn upsert_workspace_scripts<DB: Db + DbInputs>(
    db: &mut DB,
    upserts: &mut FileUpserts,
    root: Root,
    files: Vec<FileEntry>,
) {
    let other_packages: HashSet<Package> = db
        .workspace_roots()
        .roots(db)
        .iter()
        .filter(|&&other| other != root)
        .flat_map(|other| other.packages(db).iter().copied())
        .collect();

    let scripts: Vec<File> = files
        .into_iter()
        .map(|entry| {
            let package = db
                .interned_file(&entry.path)
                .and_then(|file| file.package(db))
                .filter(|pkg| other_packages.contains(pkg));
            upserts.upsert(db, package, entry)
        })
        .collect();
    if root.scripts(db) != &scripts {
        root.set_scripts(db).to(scripts);
    }
}

/// Upsert a single `File` for the watcher's
/// [`add_watched_file()`](crate::watch::add_watched_file). See [`FileUpserts`]
/// for the identity rules.
///
/// The caller must insert the returned file into a root or package container
/// for analysis to see it. This function removes it from
/// [`OrphanRoot`](oak_db::OrphanRoot).
pub(crate) fn upsert_root_file<DB: Db + DbInputs>(
    db: &mut DB,
    package: Option<Package>,
    entry: FileEntry,
) -> File {
    let mut upserts = FileUpserts::new();
    let file = upserts.upsert(db, package, entry);
    upserts.finish(db);
    file
}

/// Upserts `File` entities for a batch of entries and batches their removal
/// from [`OrphanRoot`](oak_db::OrphanRoot).
///
/// Identity comes from [`oak_db::IdentityMap`], so the entity for a path
/// doesn't depend on which containers hold it or on the order the batch
/// writes them, and a path listed twice resolves to one entity.
///
/// Callers must insert each returned [`File`] into `Package.files`,
/// `Package.scripts`, or `Root.scripts` for analysis visibility.
/// [`finish()`](Self::finish) removes the files from
/// [`OrphanRoot`](oak_db::OrphanRoot). A file
/// outside all live containers is invisible to `file_by_path()`, but the next
/// writer for its path still gets the same entity.
///
/// Batch callers write their containers before calling `finish()`, which
/// removes the whole batch from orphan with at most one membership write.
pub(crate) struct FileUpserts {
    leaving_orphan: HashSet<File>,
}

impl FileUpserts {
    pub(crate) fn new() -> Self {
        Self {
            leaving_orphan: HashSet::new(),
        }
    }

    /// Upsert a `File` for `entry`, set its `package` backpointer, and drop it
    /// from its old package.
    pub(crate) fn upsert<DB: Db + DbInputs>(
        &mut self,
        db: &mut DB,
        package: Option<Package>,
        entry: FileEntry,
    ) -> File {
        let file = db.intern_file(entry.path, entry.revision, package);

        // Rescans repair missed watcher events by updating `revision`, which
        // invalidates disk-backed `source_text()`. An editor override still
        // wins in `source_text()`, so leave it intact.
        if file.revision(db) != entry.revision {
            file.set_revision(db).to(entry.revision);
        }

        // Remove membership in the old package when classification changes,
        // or it would keep listing the file until its next rescan. The caller
        // supplies the new container, and `finish()` removes orphan membership.
        let old_package = file.package(db);
        if old_package != package {
            file.set_package(db).to(package);
            if let Some(old_pkg) = old_package {
                remove_from_pkg_files(db, old_pkg, file);
            }
        }
        self.leaving_orphan.insert(file);
        file
    }

    /// Remove the batch from `OrphanRoot` with at most one membership write.
    pub(crate) fn finish<DB: Db + DbInputs>(mut self, db: &mut DB) {
        let leaving_orphan = mem::take(&mut self.leaving_orphan);
        remove_all_from_orphan(db, &leaving_orphan);
    }
}

/// A batch dropped before `finish()` leaves its files listed in both their new
/// container and `OrphanRoot`, and no query reports the duplicate.
impl Drop for FileUpserts {
    fn drop(&mut self) {
        // A panic mid-batch already unwinds past `finish()`. Panicking again
        // here would abort the process.
        if thread::panicking() {
            return;
        }
        stdext::soft_assert!(
            self.leaving_orphan.is_empty(),
            "`FileUpserts` dropped without `finish()`"
        );
    }
}

/// Remove `file` from whichever of `pkg.files` / `pkg.scripts` holds it.
/// Used during cross-package moves: if a file's owning package changed,
/// the old package's containers still reference it until we drop the
/// entry. Also used by [`crate::watch::remove_watched_file()`] when a file
/// disappears from disk.
pub(crate) fn remove_from_pkg_files<DB: Db + DbInputs>(db: &mut DB, pkg: Package, file: File) {
    if pkg.remove_file(db, file) {
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

fn remove_all_from_orphan<DB: Db + DbInputs>(db: &mut DB, files: &HashSet<File>) {
    let orphan = db.orphan_root();
    if orphan.files(db).is_disjoint(files) {
        return;
    }
    let remaining = orphan.files(db).difference(files).copied().collect();
    orphan.set_files(db).to(remaining);
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
/// `Package`. See [`with_cow_insert()`] / [`with_cow_remove()`] for the unordered
/// `OrphanRoot` set.
pub(crate) fn with_cow_push<T: Clone + PartialEq>(files: &[T], file: T) -> Option<Vec<T>> {
    if files.contains(&file) {
        return None;
    }
    let mut updated = files.to_vec();
    updated.push(file);
    Some(updated)
}

/// The ordered container with `file` removed, or `None` if it wasn't there.
/// `None` means nothing would change, see [`with_cow_push()`].
pub(crate) fn with_cow_filter<T: Clone + PartialEq>(files: &[T], file: T) -> Option<Vec<T>> {
    if !files.contains(&file) {
        return None;
    }
    Some(files.iter().filter(|f| **f != file).cloned().collect())
}

/// The set with `item` inserted, or `None` if it's already present. The
/// unordered counterpart of [`with_cow_push()`], used for the `OrphanRoot` set
/// where membership is all that matters.
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
/// counterpart of [`with_cow_filter()`].
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
