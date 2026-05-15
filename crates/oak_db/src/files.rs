use std::sync::Arc;
use std::sync::RwLock;

use aether_url::UrlId;
use rustc_hash::FxHashMap;
use salsa::Setter;

use crate::Db;
use crate::File;
use crate::FileOwner;
use crate::Script;

/// URL-keyed registry of `File` salsa entities.
///
/// Lives on the concrete `db` (not as a salsa input) and is reachable from
/// inside tracked queries via `Db::files()`. Provides O(1) `UrlId -> File`
/// lookup.
///
/// Cloning is cheap (shared `Arc` inner). When the database is cloned for a
/// thread, the clone shares the same interner.
///
/// TODO(salsa): `remove` (for the Vfs) and `entries` (for
/// `collation_files()`) will land alongside their consumers.
///
/// # Salsa invalidation
///
/// `Files::get` takes `db` and auto-anchors on the relevant `Root`'s
/// `scripts` / `packages` fields. Tracked queries that call it get a
/// dependency on the file set under the URL's containing root, so their
/// cached results re-execute when files are added or removed under that
/// root. The anchoring is part of the lookup primitive itself; callers
/// can't accidentally forget it.
///
/// A global revision counter on `Files` would instead invalidate every
/// query that consulted it on any mutation, regardless of which URL
/// changed. ty rejected the same trade-off.
#[derive(Clone, Default)]
pub struct Files {
    inner: Arc<RwLock<FxHashMap<UrlId, File>>>,
}

impl Files {
    /// Look up the `File` interned at `url`. See [`crate::Db::file_by_url`]
    /// for the public entry point.
    ///
    /// Records salsa deps so tracked-query callers re-execute when the
    /// answer changes:
    ///
    /// - Always reads `WorkspaceRoots.roots` and `LibraryRoots.roots`
    ///   (so adding a new root that would catch this URL invalidates).
    /// - If a containing root is found, additionally reads that
    ///   `Root.scripts` and `Root.packages` (so interning/removing a
    ///   file under that root invalidates).
    ///
    /// Non-tracked callers (LSP request handlers, VFS internal logic)
    /// pay the dep-recording cost as a no-op — there's no current
    /// tracked query to attach the dep to.
    ///
    /// Generic over `Db` (rather than taking `&dyn Db`) so it can be
    /// invoked from `Db` trait default methods, where `&self` is
    /// `&Self: ?Sized` and doesn't coerce to `&dyn Db`.
    pub(crate) fn get<DB: Db + ?Sized>(&self, db: &DB, url: &UrlId) -> Option<File> {
        let workspace_roots = db.workspace_roots().roots(db);
        let library_roots = db.library_roots().roots(db);

        if let Some(path) = url.to_file_path() {
            let containing = workspace_roots
                .iter()
                .chain(library_roots.iter())
                .filter_map(|root| {
                    root.path(db)
                        .to_file_path()
                        .and_then(|p| path.starts_with(&p).then_some((p, *root)))
                })
                .max_by_key(|(p, _)| p.components().count())
                .map(|(_, r)| r);

            if let Some(root) = containing {
                let _ = root.scripts(db);
                let _ = root.packages(db);
            }
        }

        self.inner.read().unwrap().get(url).copied()
    }

    /// Look up the `Script` interned at `url`, if any. See
    /// [`crate::Db::script_by_url`] for the public entry point.
    /// Inherits the auto-anchoring of [`Files::get`].
    pub(crate) fn get_script<DB: Db + ?Sized>(&self, db: &DB, url: &UrlId) -> Option<Script> {
        match self.get(db, url)?.owner(db)? {
            FileOwner::Script(s) => Some(s),
            FileOwner::Package(_) => None,
        }
    }

    /// Insert `file` under `url`. Overwrites any previous entry for
    /// the same URL; the caller is responsible for not creating
    /// duplicate `File` entities. External callers should use
    /// [`intern_file`].
    pub(crate) fn intern(&self, url: UrlId, file: File) {
        self.inner.write().unwrap().insert(url, file);
    }
}

/// Upsert a `File` keyed by `url`.
///
/// If `url` is already interned, updates the existing `File`'s
/// `contents` and `owner` in place and returns it. Otherwise creates
/// a fresh `File::new(...)` entity and inserts it into the interner.
///
/// Replaces direct `File::new(...)` at all call sites. The idempotent
/// semantics let Vfs operations (`update_file`, `apply_scan`) be
/// upserts without an "exists vs. create" branch at every call site.
pub fn intern_file<DB: Db>(
    db: &mut DB,
    url: UrlId,
    contents: String,
    owner: Option<FileOwner>,
) -> File {
    let existing = db.files().get(db, &url);
    if let Some(file) = existing {
        file.set_contents(db).to(contents);
        file.set_owner(db).to(owner);
        return file;
    }
    let file = File::new(db, url.clone(), contents, owner);
    db.files().intern(url, file);
    file
}
