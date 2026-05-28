use std::collections::HashSet;

use aether_path::UrlId;
use oak_package_metadata::namespace::Namespace;

use crate::Db;
use crate::File;

/// Salsa-tracked root directory.
///
/// May contain top-level R scripts (typically in a Workspace root) and
/// `Package`s (from a Workspace root or a Library root), which themselves
/// wrap R `File`s.
///
/// Watchers implemented in the consumer/LSP layer are reponsible for populating
/// and keeping in sync the packages and scripts in these roots (LSP file
/// watcher for `Workspace`, custom library watcher for `Library`).
///
/// The `scripts` and `packages` fields are the salsa-observable signal for "the
/// set of scripts/packages under this root changed": tracked queries that read
/// them are invalidated when the watcher updates the corresponding list.
#[salsa::input(debug)]
pub struct Root {
    #[returns(ref)]
    pub path: UrlId,
    pub kind: RootKind,
    /// Top-level R scripts directly under this root. Each entry is a
    /// `File` with `package(db) == None`. Always empty for `Library`
    /// roots.
    ///
    /// **Placement invariant.** A file present here must have
    /// `package(db) == None`, and a file with `package == None` must
    /// live here, in another `Root.scripts`, or in
    /// `OrphanRoot.files`. Call this setter only through `oak_scan`'s
    /// helpers, which keep the back-pointer and the container in sync.
    #[returns(ref)]
    pub scripts: Vec<File>,
    /// Packages discovered under this root (workspace packages for
    /// `Workspace`, installed packages for `Library`).
    #[returns(ref)]
    pub packages: Vec<Package>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum RootKind {
    Workspace,
    Library,
}

/// A live root container that participates in analysis lookups.
///
/// Bundles the three salsa inputs that hold files / packages the user is
/// actively working with: workspace [`Root`]s, library [`Root`]s, and the
/// [`OrphanRoot`] that catches unanchored buffers. Stale entities in
/// [`StaleRoot`] aren't included -- they have separate access patterns
/// (scanner upsert only, never analysis), so they stay as their own input.
///
/// `Db::live_roots()` yields these in lookup precedence (workspace first, then
/// library, then orphan).
///
/// TODO(salsa): this enum carries the workspace-vs-library distinction in its
/// variant tag, which makes the `Root.kind` field redundant. Drop the field
/// once callers route through `LiveRoot` everywhere instead of reading
/// `root.kind(db)` directly. Further out, splitting `Root` into separate
/// `WorkspaceRoot` and `LibraryRoot` salsa inputs (each with the fields that
/// actually apply to its kind: scripts only on workspace, etc.) frees up
/// the name `Root` to be this enum.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum LiveRoot {
    Workspace(Root),
    Library(Root),
    Orphan(OrphanRoot),
}

/// The set of workspace folders the user has open.
///
/// Populated by the LSP layer from `initialize.workspaceFolders` and
/// updated on `workspace/didChangeWorkspaceFolders`. Each entry is a
/// [`Root`] of kind [`RootKind::Workspace`](crate::RootKind::Workspace).
#[salsa::input]
pub struct WorkspaceRoots {
    #[returns(ref)]
    pub roots: Vec<Root>,
}

impl WorkspaceRoots {
    /// Construct an empty `WorkspaceRoots` with no folders.
    pub fn empty(db: &dyn Db) -> Self {
        Self::new(db, vec![])
    }
}

/// The set of R libraries (`.libPaths()` entries).
///
/// Populated by the library watcher (outside the LSP since libraries live
/// outside the user's project). Each entry is a [`Root`] of kind
/// [`RootKind::Library`](crate::RootKind::Library). Order matches R's
/// `.libPaths()` lookup order.
#[salsa::input]
pub struct LibraryRoots {
    #[returns(ref)]
    pub roots: Vec<Root>,
}

impl LibraryRoots {
    /// Construct an empty `LibraryRoots` with no library directories.
    pub fn empty(db: &dyn Db) -> Self {
        Self::new(db, vec![])
    }
}

/// Files known to the database that aren't anchored to a workspace
/// or library root.
///
/// Holds: untitled buffers, files opened in the editor before the
/// workspace scanner has placed them, and any file whose URL falls
/// outside every workspace / library folder. Scanners may move files
/// out of this bucket into `Root.scripts` or `Package.files` once they
/// classify them.
///
/// Singleton: there is one `OrphanRoot` per concrete database, lazily
/// initialised by the implementation. The `files` field is what
/// [`crate::Db::file_by_url`] consults to find unanchored files.
#[salsa::input(debug)]
pub struct OrphanRoot {
    /// **Placement invariant.** Files here must have `package(db) ==
    /// None`. Call this setter only through `oak_scan`'s helpers,
    /// which keep the back-pointer and the container in sync.
    ///
    /// Unordered: these are unanchored files looked up by URL, with no
    /// collation chain among them, so membership is all that matters.
    #[returns(ref)]
    pub files: HashSet<File>,
}

impl OrphanRoot {
    pub fn empty(db: &dyn Db) -> Self {
        Self::new(db, HashSet::new())
    }
}

/// Files and packages from workspace or library roots that were removed
/// during a `set_*_paths` call.
///
/// Salsa doesn't garbage-collect entities, so dropping a `Root` doesn't
/// free its `File` and `Package` entities. They'd just leak. Instead we
/// move them here and consult this bucket on the next `set_*_paths`,
/// reusing entities by URL when their paths come back. This matters for
/// agent / multi-repo workflows where the same workspace folder gets
/// added and removed repeatedly across a session.
///
/// **Not consulted by analysis.** `Db::file_by_url` and
/// `Db::package_by_name` walk workspace / library roots and (for files)
/// `OrphanRoot` only. Entities in `StaleRoot` are invisible to
/// completions, goto-def, etc. — they correspond to folders the user
/// has explicitly removed.
///
/// **Consulted by scanners.** The scanner's package-by-URL lookup walks
/// live roots then falls back to stale. Scanner upsert helpers do the same
/// for files. On reuse, the entity is moved out of stale back into a live
/// container.
///
/// Singleton like `OrphanRoot`. The `files` and `packages` fields are
/// independent: a stale file's `package` may still point at a stale
/// package, and that's fine. Both are invisible to analysis until one
/// of them gets pulled back into a live container.
#[salsa::input]
pub struct StaleRoot {
    /// Unordered: entity-reuse storage looked up by URL, no collation chain,
    /// so membership is all that matters.
    #[returns(ref)]
    pub files: HashSet<File>,
    #[returns(ref)]
    pub packages: Vec<Package>,
}

impl StaleRoot {
    pub fn empty(db: &dyn Db) -> Self {
        Self::new(db, HashSet::new(), vec![])
    }
}

#[salsa::input(debug)]
pub struct Package {
    /// URL of the package's `DESCRIPTION` file. Stable identity across
    /// rescans and workspace / library churn: scanners look up an
    /// existing `Package` by this URL before creating a new one. Two
    /// packages with the same `Package:` name can coexist on disk and the
    /// URL distinguishes them.
    ///
    /// The package's owning [`Root`] is not stored as a field. It is
    /// derived from live-graph containment via [`Db::root_by_package`]: a
    /// package belongs to whichever `Root.packages` currently holds it.
    /// Workspace-vs-library is then `root.kind(db)`.
    #[returns(ref)]
    pub description_url: UrlId,
    // TODO(salsa): Expose a tracked `name_interned(db) -> Name<'db>`
    // method so `db.package_by_name()` and other lookups key on the
    // interned id rather than the string. Can't store `Name<'db>` on
    // `Package` directly because salsa inputs are lifetime-free.
    #[returns(ref)]
    pub name: String,
    /// Installed-package version (from `DESCRIPTION`). `None` for
    /// workspace packages.
    #[returns(ref)]
    pub version: Option<String>,
    #[returns(ref)]
    pub namespace: Namespace,
    /// R source files belonging to this package (the `R/*.R` files).
    /// Per-package granularity: adding or removing a file in one
    /// package doesn't invalidate tracked queries reading another
    /// package's files.
    ///
    /// **Placement invariant.** A file present here must have
    /// `package(db) == Some(self)`, and a file with
    /// `package == Some(self)` must live here or in [`Self::scripts`].
    /// Call this setter only through `oak_scan`'s helpers, which keep
    /// the back-pointer and the container in sync.
    #[returns(ref)]
    pub files: Vec<File>,
    /// Other R files inside the package directory that aren't part of the
    /// loadable namespace: `tests/`, `inst/`, `data-raw/`, etc. These get LSP
    /// analysis (parse, semantic index) but aren't loaded with the package, so
    /// name resolution treats them as standalone scripts that just happen to
    /// live next to the package's code.
    ///
    /// **Placement invariant.** Same as [`Self::files`]: backpointer
    /// stays `Some(self)`, file lives in one of the two containers.
    #[returns(ref)]
    pub scripts: Vec<File>,
    /// The basename ordering from `DESCRIPTION`'s `Collate` field, if
    /// present. `None` when the field is absent (R defaults to
    /// alphabetical load order). Changes only when `DESCRIPTION`
    /// itself changes, so this anchor is independent of `files` (which
    /// bumps when R/ files are added or removed).
    #[returns(ref)]
    pub collation: Option<Vec<String>>,
}
