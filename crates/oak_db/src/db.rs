use aether_url::UrlId;
use oak_package_metadata::namespace::Namespace;
use rustc_hash::FxHashMap;
use salsa::Setter;

use crate::File;
use crate::LibraryRoots;
use crate::OrphanRoot;
use crate::Package;
use crate::Root;
use crate::WorkspaceRoots;

/// Salsa Database trait.
///
/// Defines the abstract surface that tracked queries in this crate
/// consume. The canonical concrete implementation is [`crate::OakDatabase`]
/// (same crate). When future db-trait crates land (e.g. an
/// `oak_types::Db: oak_db::Db`), they add their `impl` for
/// `OakDatabase` externally via the orphan rule.
///
/// `WorkspaceRoots`, `LibraryRoots`, and `OrphanRoot` are meant to be
/// singletons. Concrete dbs lazy-init them (typically via
/// `Arc<OnceLock<_>>`).
#[salsa::db]
pub trait Db: salsa::Database {
    /// Workspace folders opened by the editor.
    fn workspace_roots(&self) -> WorkspaceRoots;

    /// R library roots (entries in `.libPaths()`).
    fn library_roots(&self) -> LibraryRoots;

    /// Files not yet anchored to any workspace or library root.
    fn orphan_root(&self) -> OrphanRoot;

    /// Look up the `File` interned at `url`, if any.
    ///
    /// Salsa-cached: the first call walks the per-root indices, the
    /// rest hit the cache until a relevant root's `scripts` /
    /// `packages` / per-package `files` (or `orphan_root().files`)
    /// changes. `where Self: Sized` so the default body can dispatch
    /// to a salsa tracked function that takes `&dyn Db`; `&dyn Db`
    /// callers go through [`file_by_url_query`] directly.
    fn file_by_url(&self, url: &UrlId) -> Option<File>
    where
        Self: Sized,
    {
        file_by_url_query(self, url)
    }

    /// Look up the `Package` named `name`, applying R's precedence:
    /// workspace packages shadow installed ones; within each group,
    /// declaration order wins.
    fn package_by_name(&self, name: &str) -> Option<Package>
    where
        Self: Sized,
    {
        package_by_name_query(self, name)
    }

    /// Upsert a `File` keyed by `url`.
    ///
    /// If a file is already known at `url`, updates its `contents` and
    /// `package` fields in place. Otherwise creates a new `File` and
    /// places it in `package.files` (when `package` is `Some`) or in
    /// `orphan_root().files` (when `package` is `None`).
    ///
    /// Update does *not* relocate an existing file between buckets. If
    /// a scanner later determines a different home (e.g. a workspace
    /// top-level script that should live in `root.scripts`), it mutates
    /// the relevant input directly.
    fn set_file(&mut self, url: UrlId, contents: String, package: Option<Package>) -> File
    where
        Self: Sized,
    {
        if let Some(file) = self.file_by_url(&url) {
            file.set_contents(self).to(contents);
            file.set_package(self).to(package);
            return file;
        }
        let file = File::new(self, url, contents, package);
        match package {
            Some(pkg) => {
                let mut files = pkg.files(self).clone();
                files.push(file);
                pkg.set_files(self).to(files);
            },
            None => {
                let orphan = self.orphan_root();
                let mut files = orphan.files(self).clone();
                files.push(file);
                orphan.set_files(self).to(files);
            },
        }
        file
    }

    /// Upsert a `Package` keyed by `(root, name)`.
    ///
    /// If an existing package matches, updates its fields in place.
    /// Otherwise creates a new `Package` and appends it to
    /// `root.packages`. Idempotent: a scanner can call this on every
    /// rescan.
    fn set_package(
        &mut self,
        root: Root,
        name: String,
        version: Option<String>,
        namespace: Namespace,
        files: Vec<File>,
        collation: Option<Vec<String>>,
    ) -> Package
    where
        Self: Sized,
    {
        let existing = root
            .packages(self)
            .iter()
            .find(|p| p.name(self) == &name)
            .copied();
        if let Some(pkg) = existing {
            pkg.set_version(self).to(version);
            pkg.set_namespace(self).to(namespace);
            pkg.set_files(self).to(files);
            pkg.set_collation(self).to(collation);
            return pkg;
        }
        let pkg = Package::new(self, root, name, version, namespace, files, collation);
        let mut packages = root.packages(self).clone();
        packages.push(pkg);
        root.set_packages(self).to(packages);
        pkg
    }
}

/// Implementation of [`Db::file_by_url`]. Walks the per-root indices.
///
/// Not itself a salsa-tracked function (its `&UrlId` argument isn't a
/// salsa entity), but every step is: each [`root_url_index`] call
/// returns a cached map. Adding a file to one root invalidates only
/// that root's index. `pub(crate)` so `&dyn Db` callers inside the
/// crate (e.g. the resolver) can use it without the trait method's
/// `Self: Sized` bound; downstream code dispatches via the trait
/// method.
pub(crate) fn file_by_url_query(db: &dyn Db, url: &UrlId) -> Option<File> {
    for root in db.workspace_roots().roots(db) {
        if let Some(&file) = root_url_index(db, *root).get(url) {
            return Some(file);
        }
    }
    for root in db.library_roots().roots(db) {
        if let Some(&file) = root_url_index(db, *root).get(url) {
            return Some(file);
        }
    }
    orphan_url_index(db).get(url).copied()
}

/// Implementation of [`Db::package_by_name`]. Same shape as
/// [`file_by_url_query`].
pub(crate) fn package_by_name_query(db: &dyn Db, name: &str) -> Option<Package> {
    for root in db.workspace_roots().roots(db) {
        if let Some(&pkg) = root_package_index(db, *root).get(name) {
            return Some(pkg);
        }
    }
    for root in db.library_roots().roots(db) {
        if let Some(&pkg) = root_package_index(db, *root).get(name) {
            return Some(pkg);
        }
    }
    None
}
/// Per-root URL → File index. Salsa caches one map per `Root`;
/// reads only `root.scripts`, `root.packages`, and each
/// `pkg.files` reachable from this root. Adding or removing a file
/// in *this* root invalidates this entry; other roots stay cached.
#[salsa::tracked(returns(ref))]
fn root_url_index(db: &dyn Db, root: Root) -> FxHashMap<UrlId, File> {
    let mut map = FxHashMap::default();
    for &file in root.scripts(db) {
        map.insert(file.url(db).clone(), file);
    }
    for &pkg in root.packages(db) {
        for &file in pkg.files(db) {
            map.insert(file.url(db).clone(), file);
        }
    }
    map
}

/// Orphan URL → File index. Reads only `orphan_root().files`.
#[salsa::tracked(returns(ref))]
fn orphan_url_index(db: &dyn Db) -> FxHashMap<UrlId, File> {
    let mut map = FxHashMap::default();
    for &file in db.orphan_root().files(db) {
        map.insert(file.url(db).clone(), file);
    }
    map
}

/// Per-root name → Package index. Same granularity as
/// [`root_url_index`].
#[salsa::tracked(returns(ref))]
fn root_package_index(db: &dyn Db, root: Root) -> FxHashMap<String, Package> {
    let mut map = FxHashMap::default();
    for &pkg in root.packages(db) {
        map.insert(pkg.name(db).clone(), pkg);
    }
    map
}
