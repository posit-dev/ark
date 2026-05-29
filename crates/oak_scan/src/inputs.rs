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
//! Placement is single-source-of-truth: a file belongs to whichever
//! container Vec holds it (`pkg.files`, `root.scripts`, or
//! `orphan_root().files`), and `File::package` derives ownership from
//! that. These helpers keep a file in exactly one container as it moves,
//! so the derived lookup stays unambiguous.

use std::collections::HashSet;
use std::path::PathBuf;

use aether_url::UrlId;
use oak_db::stale_file_by_url;
use oak_db::Db;
use oak_db::DbInputs;
use oak_db::File;
use oak_db::Package;
use oak_db::Root;
use oak_package_metadata::namespace::Namespace;
use salsa::Setter;

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
pub trait DbExt: Db + DbInputs {
    /// Reconcile `LibraryRoots` to exactly `paths`.
    ///
    /// - Paths already present as a `Root`: untouched. No fs walk, no
    ///   salsa churn.
    /// - New paths: scanned and added.
    /// - Removed paths: their `Root` is dropped and the contained `File`
    ///   and `Package` entities move to [`oak_db::StaleRoot`] so that
    ///   a later call that brings the same path back reuses the same
    ///   entities (Salsa never GCs them since they are inputs).
    ///
    /// Order in `LibraryRoots.roots` follows `paths`, matching R's
    /// `.libPaths()` precedence.
    fn set_library_paths(&mut self, paths: &[PathBuf]);
}

impl<DB: Db + DbInputs> DbExt for DB {
    fn set_library_paths(&mut self, paths: &[PathBuf]) {
        crate::library::set_library_paths(self, paths);
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
        collation: Option<Vec<String>>,
    ) -> Package {
        // `package_by_url()` finds the existing entity whether it's already
        // in `self.packages` (rescan, common path) or in
        // `stale_root.packages` (resurrection after a previous eviction).
        // Either way we reuse the entity and refresh its metadata fields.
        let pkg = match db.package_by_url(&description_url) {
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
                collation,
            ),
        };

        let file_entities: Vec<File> = files
            .into_iter()
            .map(|entry| upsert_file(db, Some(pkg), entry))
            .collect();

        pkg.set_files(db).to(file_entities);
        pkg
    }

    fn set_stale<DB: Db + DbInputs>(self, db: &mut DB, editor_owned: Option<&HashSet<UrlId>>) {
        crate::stale::set_root_stale(db, self, editor_owned);
    }
}

fn upsert_file<DB: Db + DbInputs>(db: &mut DB, package: Option<Package>, entry: FileEntry) -> File {
    if let Some(old) = db.file_by_url(&entry.url) {
        // The caller will place this file in `package`'s `files` vec. Two
        // cleanups keep the file in exactly one live container, so the
        // derived `File::package` stays unambiguous:
        //
        // - If the file currently belongs to a *different* package, drop it
        //   from that package's `files` vec; otherwise both would list it.
        //   Normally a no-op, since a file's package is fixed by its path:
        //   this only fires in the pathological nested-root case where two
        //   packages claim the same URL.
        //
        // - If the file was in `OrphanRoot.files` (typically because the
        //   editor had it open before a scan classified it), remove it.
        let old_package = old.package(db);
        if old_package != package {
            if let Some(old_pkg) = old_package {
                remove_from_pkg_files(db, old_pkg, old);
            }
        }
        remove_from_orphan(db, old);
        return old;
    }

    if let Some(stale) = stale_file_by_url(db, &entry.url) {
        // Resurrecting an evicted File. Restore disk contents (the editor-owned
        // variant lives in `orphan_root` instead; this branch only sees
        // scanner-discovered files). The caller places it into `package.files`.
        stale.set_contents(db).to(entry.contents);
        remove_from_stale_files(db, stale);
        return stale;
    }

    File::new(db, entry.url, entry.contents)
}

fn remove_from_pkg_files<DB: Db + DbInputs>(db: &mut DB, pkg: Package, file: File) {
    if !pkg.files(db).contains(&file) {
        return;
    }
    let mut files = pkg.files(db).clone();
    files.retain(|f| *f != file);
    pkg.set_files(db).to(files);
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

fn remove_from_stale_files<DB: Db + DbInputs>(db: &mut DB, file: File) {
    let stale = db.stale_root();
    if !stale.files(db).contains(&file) {
        return;
    }
    let mut files = stale.files(db).clone();
    files.retain(|f| *f != file);
    stale.set_files(db).to(files);
}

fn remove_from_stale_packages<DB: Db + DbInputs>(db: &mut DB, pkg: Package) {
    let stale = db.stale_root();
    if !stale.packages(db).contains(&pkg) {
        return;
    }
    let mut packages = stale.packages(db).clone();
    packages.retain(|p| *p != pkg);
    stale.set_packages(db).to(packages);
}
