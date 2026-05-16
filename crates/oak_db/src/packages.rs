use std::sync::Arc;
use std::sync::RwLock;

use compact_str::CompactString;
use oak_package_metadata::namespace::Namespace;
use rustc_hash::FxHashMap;
use salsa::Setter;

use crate::Db;
use crate::File;
use crate::Package;
use crate::Root;

/// (Root, name) -> Package registry.
///
/// Lives on the concrete `db` (not as a salsa input) and is reachable from
/// inside tracked queries via the [`crate::Db::package_by_name`] lookup.
/// Provides O(1) by-name lookup, scoped per `Root` so the precedence walk
/// can anchor lazily on only the roots it touches.
///
/// Cloning is cheap (shared `Arc` inner). When the database is cloned for
/// a thread, the clone shares the same interner.
///
/// # Salsa invalidation
///
/// [`Packages::get`] auto-anchors on each `Root.packages` field it walks
/// (workspace roots in declaration order, then library roots in
/// `.libPaths()` order). Salsa records deps only on the roots actually
/// touched, so a workspace match doesn't depend on library roots.
#[derive(Clone, Default)]
pub struct Packages {
    by_root: Arc<RwLock<FxHashMap<Root, FxHashMap<CompactString, Package>>>>,
}

impl Packages {
    /// Look up the `Package` named `name` using R's precedence rule:
    /// workspace packages shadow installed ones; within each group,
    /// declaration order wins.
    ///
    /// Anchors lazily on each root walked; returns at the first hit so
    /// later roots aren't read (and don't appear in the dep set).
    ///
    /// Generic over `Db` (rather than `&dyn Db`) so the [`crate::Db`]
    /// trait can call this from a default-impl method, where `&self` is
    /// `&Self: ?Sized` and doesn't coerce to `&dyn Db`.
    pub(crate) fn get<DB: Db + ?Sized>(&self, db: &DB, name: &str) -> Option<Package> {
        for root in db.workspace_roots().roots(db) {
            let _ = root.packages(db);
            if let Some(pkg) = self.lookup(*root, name) {
                return Some(pkg);
            }
        }
        for root in db.library_roots().roots(db) {
            let _ = root.packages(db);
            if let Some(pkg) = self.lookup(*root, name) {
                return Some(pkg);
            }
        }
        None
    }

    /// Bare hashmap lookup. No salsa dep recorded.
    fn lookup(&self, root: Root, name: &str) -> Option<Package> {
        self.by_root
            .read()
            .unwrap()
            .get(&root)
            .and_then(|m| m.get(name))
            .copied()
    }

    /// Insert `package` under `(root, name)`. Overwrites any previous
    /// entry, callers should not create duplicate `Package` entities.
    /// External callers go through [`intern_package`].
    pub(crate) fn intern(&self, root: Root, name: CompactString, package: Package) {
        self.by_root
            .write()
            .unwrap()
            .entry(root)
            .or_default()
            .insert(name, package);
    }
}

/// Upsert a `Package` keyed by `(root, name)`.
///
/// If a package with this `(root, name)` is already interned, updates
/// the existing entity's fields in place and returns it. Otherwise
/// creates a fresh `Package::new(...)` and inserts it into the
/// interner. Idempotent: the watcher can call this on every scan.
pub fn intern_package<DB: Db>(
    db: &mut DB,
    root: Root,
    name: String,
    version: Option<String>,
    namespace: Namespace,
    files: Vec<File>,
    collation: Option<Vec<String>>,
) -> Package {
    let key = CompactString::from(name.as_str());
    if let Some(pkg) = db.packages().lookup(root, &key) {
        pkg.set_version(db).to(version);
        pkg.set_namespace(db).to(namespace);
        pkg.set_files(db).to(files);
        pkg.set_collation(db).to(collation);
        return pkg;
    }
    let pkg = Package::new(db, root, name, version, namespace, files, collation);
    db.packages().intern(root, key, pkg);
    pkg
}
