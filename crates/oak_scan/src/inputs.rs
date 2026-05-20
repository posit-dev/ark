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

use std::path::PathBuf;

use aether_url::UrlId;
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
    /// Scan each path in `paths` and register the discovered packages
    /// under `LibraryRoots`. Existing `Root` entities at each path are
    /// reused so downstream queries that depend on `Root` identity
    /// stay cached across rescans. Order in `LibraryRoots.roots`
    /// mirrors `paths`, matching R's `.libPaths()` lookup precedence.
    fn scan_library_paths(&mut self, paths: &[PathBuf]);
}

impl<DB: Db + DbInputs> DbExt for DB {
    fn scan_library_paths(&mut self, paths: &[PathBuf]) {
        crate::library::scan_library_paths(self, paths);
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
    /// Identity is keyed on `(self, DESCRIPTION name)`: if
    /// `self.packages` already contains a `Package` with `name`, that
    /// entity is reused and its version / namespace / collation fields
    /// are updated in place. Salsa backdates each setter call when the
    /// value doesn't actually change.
    ///
    /// Files are reused by URL via [`Db::file_by_url`]; see
    /// [`FileEntry`] for the content-preservation semantics. Wiring
    /// the returned `Package` into `self.packages` is the caller's
    /// job.
    fn set_package<DB: Db + DbInputs>(
        self,
        db: &mut DB,
        name: String,
        version: Option<String>,
        namespace: Namespace,
        files: Vec<FileEntry>,
        collation: Option<Vec<String>>,
    ) -> Package;
}

impl RootExt for Root {
    fn set_package<DB: Db + DbInputs>(
        self,
        db: &mut DB,
        name: String,
        version: Option<String>,
        namespace: Namespace,
        files: Vec<FileEntry>,
        collation: Option<Vec<String>>,
    ) -> Package {
        let existing = self
            .packages(db)
            .iter()
            .find(|p| p.name(db) == &name)
            .copied();

        let pkg = match existing {
            Some(p) => {
                p.set_version(db).to(version);
                p.set_namespace(db).to(namespace);
                p.set_collation(db).to(collation);
                p
            },
            None => Package::new(db, self, name, version, namespace, Vec::new(), collation),
        };

        let file_entities: Vec<File> = files
            .into_iter()
            .map(|entry| upsert_file(db, Some(pkg), entry))
            .collect();

        pkg.set_files(db).to(file_entities);
        pkg
    }
}

fn upsert_file<DB: Db + DbInputs>(db: &mut DB, package: Option<Package>, entry: FileEntry) -> File {
    match db.file_by_url(&entry.url) {
        Some(existing) => {
            existing.set_package(db).to(package);
            existing
        },
        None => File::new(db, entry.url, entry.contents, package),
    }
}
