use std::collections::HashSet;

use aether_path::FilePath;

use crate::Db;
use crate::File;
use crate::Package;

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
    pub path: FilePath,
    #[returns(copy)]
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
    /// Directories containing `.Rprofile` (renv, rv) or `.Renviron` (rig).
    /// Relative `source()` paths anchor at the nearest enclosing entry,
    /// assuming a session started there to load that environment.
    ///
    /// Sorted by path so unchanged rescans compare equal and reverse lookup
    /// finds the nearest enclosing directory first. Always empty for `Library` roots.
    #[returns(ref)]
    pub environment_dirs: Vec<FilePath>,
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
/// [`OrphanRoot`] that catches unanchored buffers.
///
/// `crate::SourceDb::live_roots()` yields these in lookup precedence (workspace first, then
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

/// The merged library roots of all live environments.
///
/// `oak_scan`'s environment writers maintain this list. Each entry is a
/// [`Root`] of kind [`RootKind::Library`]. Roots appear once, in order of first
/// occurrence across [`WorkspaceEnvironments`], followed by the default
/// environment. Each environment's roots follow its R session's `.libPaths()`
/// lookup order. Library watchers rescan these roots without changing the list.
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

/// Library resolution state for files that share an R startup directory.
///
/// Workspace environments represent roots and their environment directories
/// ([`Root::environment_dirs()`]). [`WorkspaceEnvironments`] lists the live
/// ones. The always-live default has no workspace directory.
/// [`File::environment()`](crate::File::environment) selects an environment
/// for each file.
///
/// **Write invariant.** [`LibraryRoots`] contains each live environment's
/// library roots once, in order of first occurrence across
/// [`WorkspaceEnvironments`], then the default. Update `status` only through
/// `oak_scan`'s environment writers, which maintain this list and evict roots
/// that no live environment lists.
#[salsa::input(debug)]
pub struct Environment {
    /// R startup directory for a workspace environment. `None` for the
    /// default, which is not tied to a workspace directory.
    #[returns(ref)]
    pub dir: Option<FilePath>,
    #[returns(ref)]
    pub status: EnvironmentStatus,
    /// Error from the last failed run, cleared on success or when the default
    /// is marked unresolved. A failed run preserves `status`, so a failed
    /// rerun can retain libraries from the last success.
    #[returns(ref)]
    pub last_error: Option<String>,
}

impl Environment {
    pub(crate) fn new_default(db: &dyn Db) -> Self {
        Self::new(db, None, EnvironmentStatus::Resolved(Vec::new()), None)
    }

    /// Whether this is the default environment or a member of
    /// [`WorkspaceEnvironments`]. Removed environments remain allocated
    /// because Salsa does not garbage-collect inputs, so a resolution started
    /// before a removal can complete for an environment that is no longer live.
    pub fn is_live(self, db: &dyn Db) -> bool {
        self == db.default_environment() ||
            db.workspace_environments().environments(db).contains(&self)
    }
}

/// Whether library roots are available for an [`Environment`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum EnvironmentStatus {
    /// No library roots are available. Workspace environments start here.
    /// Marking the default unresolved or removing a workspace environment
    /// also resets it to this state, even after a successful run.
    /// See [`Environment::last_error()`] for a failed run.
    Unresolved,
    /// Library roots from the last successful resolution, in `.libPaths()`
    /// lookup order. Environments that report the same path share one [`Root`].
    /// The default starts in this state with an empty list, before any run.
    Resolved(Vec<Root>),
}

impl EnvironmentStatus {
    /// The resolved library roots, empty when unresolved.
    pub fn library_roots(&self) -> &[Root] {
        match self {
            EnvironmentStatus::Unresolved => &[],
            EnvironmentStatus::Resolved(roots) => roots,
        }
    }
}

/// The live environments of workspace roots and their environment
/// directories, one per directory. Excludes the default environment.
///
/// List order determines library precedence in [`LibraryRoots`].
/// `oak_scan`'s `DbScan::reconcile_environments()` orders workspace roots as listed in
/// [`WorkspaceRoots`], each followed by its environment directories sorted by
/// path. Directories shared by roots appear only at their first occurrence.
#[salsa::input]
pub struct WorkspaceEnvironments {
    #[returns(ref)]
    pub environments: Vec<Environment>,
}

impl WorkspaceEnvironments {
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
/// [`crate::SourceDb::file_by_path`] consults to find unanchored files.
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

/// A notebook whose cells the editor syncs through `notebookDocument/*`
/// notifications. Also covers Quarto and R Markdown documents, which Positron
/// presents as notebooks.
///
/// Each R cell is an ordinary editor [`File`]. The `cells` list records
/// document order, not execution history. [`crate::load_context`] uses that
/// order to determine cross-cell visibility.
#[salsa::input(debug)]
pub struct Notebook {
    #[returns(ref)]
    pub path: FilePath,
    #[returns(ref)]
    pub cells: Vec<File>,
}

/// The notebooks currently open in the editor.
///
/// Closed [`Notebook`] inputs remain allocated because Salsa does not
/// garbage-collect inputs. Unlike `File` and `Package`, notebooks have no entry
/// in [`crate::IdentityMap`], so reopening a notebook allocates a new input.
#[salsa::input]
pub struct OpenNotebooks {
    #[returns(ref)]
    pub notebooks: Vec<Notebook>,
}

impl OpenNotebooks {
    pub fn empty(db: &dyn Db) -> Self {
        Self::new(db, vec![])
    }
}
