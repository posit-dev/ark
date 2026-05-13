use std::sync::Arc;
use std::sync::RwLock;

use aether_url::UrlId;
use rustc_hash::FxHashMap;
use salsa::Setter;

use crate::Db;
use crate::File;
use crate::SourceNode;

/// URL-keyed registry of `File` salsa entities.
///
/// Lives on the concrete `db` (not as a salsa input) and is reachable from
/// inside tracked queries via `Db::files()`. Provides O(1) `UrlId -> File`
/// lookup.
///
/// Cloning is cheap (shared `Arc` inner). When the database is cloned for a
/// thread, the clone shares the same interner.
///
/// The struct itself is `pub` because [`crate::Db`] implementors hold a `Files`
/// field. Its primitives (`get`, `intern`) are `pub(crate)`. External callers
/// go through [`intern_file`] for upserts. Reads happen through role-aware
/// helpers (such as `SourceGraph::script_by_url`) that anchor properly for
/// tracked queries.
///
/// TODO(salsa): `remove` (for the Vfs) and `entries` (for
/// `collation_files()`) will land alongside their consumers.
///
/// # Salsa invalidation
///
/// `Files::get` records no salsa dep. Lookups must be anchored at the caller.
///
/// `SourceGraph::script_by_url()` reads `Root.revision` for the URL's
/// containing root, or `WorkspaceRoots.roots` for orphan URLs. This gives
/// tracked-query callers a dependency on the file set, so their cached results
/// re-execute when files are added or removed.
///
/// A global revision counter on `Files` would invalidate every query that
/// consulted it on any mutation, regardless of which URL changed. ty rejected
/// the same trade-off.
#[derive(Clone, Default)]
pub struct Files {
    inner: Arc<RwLock<FxHashMap<UrlId, File>>>,
}

impl Files {
    pub(crate) fn get(&self, url: &UrlId) -> Option<File> {
        self.inner.read().unwrap().get(url).copied()
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
/// `contents` and `parent` in place and returns it. Otherwise creates
/// a fresh `File::new(...)` entity and inserts it into the interner.
///
/// Replaces direct `File::new(...)` at all call sites. The idempotent
/// semantics let Vfs operations (`update_file`, `apply_scan`) be
/// upserts without an "exists vs. create" branch at every call site.
pub fn intern_file<DB: Db>(
    db: &mut DB,
    url: UrlId,
    contents: String,
    parent: Option<SourceNode>,
) -> File {
    let existing = db.files().get(&url);
    if let Some(file) = existing {
        file.set_contents(db).to(contents);
        file.set_parent(db).to(parent);
        return file;
    }
    let file = File::new(db, url.clone(), contents, parent);
    db.files().intern(url, file);
    file
}
