use std::collections::hash_map::Entry;
use std::io;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::PoisonError;

use aether_path::FilePath;
use camino::Utf8Path;
use indexmap::IndexSet;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use salsa::Setter;

use crate::Environment;
use crate::File;
use crate::FileRevision;
use crate::LibraryRoots;
use crate::LiveRoot;
use crate::Notebook;
use crate::OpenNotebooks;
use crate::OrphanRoot;
use crate::Package;
use crate::PackageFiles;
use crate::Root;
use crate::WorkspaceEnvironments;
use crate::WorkspaceRoots;

/// Concrete-input surface of the salsa database. Each impl
/// ([`crate::OakDatabase`], the test db) supplies singleton input handles
/// and file reads.
///
/// [`SourceDb`] adds lookups over these inputs. [`Db`] additionally permits
/// recursive semantic queries.
#[salsa::db]
pub trait DbInputs: salsa::Database {
    /// Workspace folders opened by the editor.
    fn workspace_roots(&self) -> WorkspaceRoots;

    /// R library roots (entries in `.libPaths()`).
    fn library_roots(&self) -> LibraryRoots;

    /// Files not yet anchored to any workspace or library root.
    fn orphan_root(&self) -> OrphanRoot;

    /// Live environments of workspace roots and their environment
    /// directories. See [`WorkspaceEnvironments`].
    fn workspace_environments(&self) -> WorkspaceEnvironments;

    /// The always-live fallback for library files, virtual documents and
    /// files with no live workspace environment. See [`File::environment()`].
    ///
    /// Starts resolved to no libraries, so databases that do not manage
    /// environments have a resolved environment for every file.
    fn default_environment(&self) -> Environment;

    /// The [`IdentityMap`] shared by all handles to this database.
    fn identity_map(&self) -> &IdentityMap;

    /// Read through the database's file reader. Note that this method is not
    /// tracked and callers _must_ depend on the corresponding revision input
    /// for the file before reading.
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String>;

    /// Notebooks open in the editor, with their cells in document order.
    fn open_notebooks(&self) -> OpenNotebooks;

    /// Write-side buffer registry, excluded from Salsa queries. See [`EditorBuffers`].
    fn editor_buffers_mut(&mut self) -> &mut EditorBuffers;

    /// Record `file` as an open editor buffer with `contents`.
    ///
    /// This does not establish container membership, so the file may remain
    /// invisible to analysis. Use `oak_scan::DbScan::upsert_editor()` to also
    /// make the buffer visible.
    fn set_editor_buffer(&mut self, file: File, contents: String) {
        file.set_source_text_override(self).to(Some(contents));
        self.editor_buffers_mut().insert(file);
    }

    /// Release the editor buffer of `file`, making disk contents
    /// authoritative.
    ///
    /// Container membership is unchanged. Use `oak_scan::DbScan::close_editor()`
    /// to also remove orphan membership.
    fn clear_editor_buffer(&mut self, file: File) {
        // Repeated closes must not advance the Salsa revision. `IdentityMap`
        // retains closed entities, so they can reach this method again.
        if file.source_text_override(self).is_some() {
            file.set_source_text_override(self).to(None);
        }
        self.editor_buffers_mut().remove(file);
    }

    /// Files with an open editor buffer, in opening order.
    fn editor_buffer_files(&mut self) -> Vec<File> {
        self.editor_buffers_mut().files()
    }

    /// Whether `file` has an open editor buffer.
    fn is_editor_buffer(&mut self, file: File) -> bool {
        self.editor_buffers_mut().contains(file)
    }

    /// Look up the [`File`] for `path` without creating it or checking live
    /// membership. Analysis uses [`SourceDb::file_by_path()`] instead.
    ///
    /// Takes `&mut self` so tracked queries, which only get shared references,
    /// can't call it. See [`IdentityMap`].
    fn interned_file(&mut self, path: &FilePath) -> Option<File> {
        self.identity_map().file(path)
    }

    /// Look up the [`Package`] for `description_path` without creating it or
    /// checking live membership.
    ///
    /// Takes `&mut self` for the same reason as [`Self::interned_file()`].
    fn interned_package(&mut self, description_path: &FilePath) -> Option<Package> {
        self.identity_map().package(description_path)
    }

    /// Return the [`File`] for `path`, creating it from `revision` and `package`
    /// if absent. Existing entities are returned unchanged, so callers must
    /// update any fields that differ.
    ///
    /// This does not add the file to a container. Callers must establish
    /// membership for analysis to see it.
    fn intern_file(
        &mut self,
        path: FilePath,
        revision: FileRevision,
        package: Option<Package>,
    ) -> File {
        self.identity_map()
            .intern_file(self, path, revision, package)
    }

    /// Return the [`Package`] for `description_path`, creating it from the
    /// metadata arguments if absent. Existing entities are returned unchanged.
    /// Callers must update their fields and establish root membership, as with
    /// [`Self::intern_file()`].
    fn intern_package(
        &mut self,
        description_path: FilePath,
        name: String,
        description_revision: FileRevision,
        namespace_revision: FileRevision,
        index_revision: Option<FileRevision>,
    ) -> Package {
        self.identity_map().intern_package(
            self,
            description_path,
            name,
            description_revision,
            namespace_revision,
            index_revision,
        )
    }
}

/// Files with an open editor buffer, in opening order.
///
/// [`DbInputs::set_editor_buffer()`] and [`DbInputs::clear_editor_buffer()`]
/// update membership and `File.source_text_override` together. Callers must use
/// these methods rather than raw override setters to keep the registry consistent.
///
/// This is write-side state, not a Salsa input. Access through [`DbInputs`]
/// requires `&mut self`, so queries cannot depend on buffer membership.
/// Read-only database snapshots start with an empty registry.
#[derive(Clone, Default)]
pub struct EditorBuffers {
    files: IndexSet<File>,
}

impl EditorBuffers {
    fn insert(&mut self, file: File) {
        self.files.insert(file);
    }

    fn remove(&mut self, file: File) {
        self.files.shift_remove(&file);
    }

    fn files(&self) -> Vec<File> {
        self.files.iter().copied().collect()
    }

    fn contains(&self, file: File) -> bool {
        self.files.contains(&file)
    }
}

/// One [`File`] per path and one [`Package`] per `DESCRIPTION` path for the
/// database's lifetime.
///
/// Writers reuse entities through this map independently of container
/// membership. Eviction, rescans, and buffer closure can remove membership
/// without changing the entity returned to the next writer for that path.
/// Analysis visibility depends on live containers, as checked by
/// [`SourceDb::file_by_path()`].
///
/// This is not a Salsa input, so map updates do not invalidate queries. A query
/// reading it would get an untracked answer that depends on which paths were
/// ever interned, not on live membership. Queries outside this module can't
/// read it: the map's methods are private to this module, and every
/// [`DbInputs`] method that reaches them takes `&mut self`, which queries never
/// get. Queries defined in this module must not call the private lookups
/// through [`DbInputs::identity_map()`]. Entries are
/// retained for the database's lifetime,
/// matching Salsa's retention of input entities. The map stores paths and
/// handles to those entities.
///
/// Salsa generates a public constructor for every input, so the compiler can't
/// enforce creation through this map. Production code creates `File` and
/// `Package` entities only through [`DbInputs::intern_file()`] and
/// [`DbInputs::intern_package()`]. A fixture that calls `File::new()` directly
/// and then invokes an `oak_scan` writer on the same path creates two entities.
#[derive(Default)]
pub struct IdentityMap {
    inner: Mutex<Identities>,
}

#[derive(Default)]
struct Identities {
    files: FxHashMap<FilePath, File>,
    packages: FxHashMap<FilePath, Package>,
}

impl IdentityMap {
    fn file(&self, path: &FilePath) -> Option<File> {
        self.lock().files.get(path).copied()
    }

    fn package(&self, description_path: &FilePath) -> Option<Package> {
        self.lock().packages.get(description_path).copied()
    }

    fn intern_file<DB: ?Sized + salsa::Database>(
        &self,
        db: &DB,
        path: FilePath,
        revision: FileRevision,
        package: Option<Package>,
    ) -> File {
        match self.lock().files.entry(path) {
            Entry::Occupied(entry) => *entry.get(),
            Entry::Vacant(entry) => {
                let file = File::new(db, entry.key().clone(), revision, None, package);
                *entry.insert(file)
            },
        }
    }

    fn intern_package<DB: ?Sized + salsa::Database>(
        &self,
        db: &DB,
        description_path: FilePath,
        name: String,
        description_revision: FileRevision,
        namespace_revision: FileRevision,
        index_revision: Option<FileRevision>,
    ) -> Package {
        match self.lock().packages.entry(description_path) {
            Entry::Occupied(entry) => *entry.get(),
            Entry::Vacant(entry) => {
                let pkg = Package::new(
                    db,
                    entry.key().clone(),
                    name,
                    description_revision,
                    namespace_revision,
                    index_revision,
                    None,
                    PackageFiles::default(),
                    Vec::new(),
                );
                *entry.insert(pkg)
            },
        }
    }

    // A poisoned lock can be recovered because each critical section only
    // reads the maps or inserts a handle after constructing its entity. A
    // constructor panic leaves no entry pointing to an incomplete entity.
    fn lock(&self) -> MutexGuard<'_, Identities> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Database access for source text, syntax, membership, and package metadata.
/// Queries accepting this trait cannot directly call semantic queries that
/// require [`Db`].
///
/// Lookup methods delegate to tracked per-root indices, preserving their
/// invalidation boundaries. Recovery helpers also use this trait: keep its
/// implementations and their transitive dependencies free of semantic queries.
#[salsa::db]
pub trait SourceDb: DbInputs {
    /// Look up the `File` interned at `path`, if any.
    ///
    /// Walks the per-root URL indices in workspace-then-library order,
    /// then falls back to the orphan bucket. The walk short-circuits
    /// on the first hit, so callers depend only on the index maps
    /// actually visited.
    fn file_by_path(&self, path: &FilePath) -> Option<File>;

    /// Look up the `Package` named `name`, applying the following precedence:
    /// - Workspace packages shadow installed ones
    /// - Installed packages in an earlier root shadow later ones
    ///   (mirroring `.libPaths()`).
    fn package_by_name(&self, name: &str) -> Option<Package>;

    /// Resolve the live `Root` that contains `pkg`, if any.
    ///
    /// Returns `None` when no live root lists the package, including when all
    /// roots that listed it are evicted.
    ///
    /// **Nested roots.** Two roots can claim the same package when one is
    /// nested inside the other, e.g. the frontend opens both `/proj` and
    /// `/proj/sub-pkg` as workspace folders and both scans walk into
    /// `sub-pkg/DESCRIPTION`. Both scans hand the same `Package` entity to
    /// their respective root's `packages` vec, and both vecs keep listing
    /// it for as long as both folders are open (the outer scan re-walks
    /// into `sub-pkg` every time). The overlap is steady state, not
    /// transient, so this query resolves it by picking the longest-path
    /// root. `root_by_file` applies the same rule at the file level.
    fn root_by_package(&self, pkg: Package) -> Option<Root>;

    /// All live roots in lookup-precedence order: workspace folders first, then
    /// library paths (mirroring R's `.libPaths()`), then the orphan bucket.
    /// Salsa caches this list. Changes to `WorkspaceRoots.roots` or
    /// `LibraryRoots.roots` invalidate it.
    fn live_roots(&self) -> &[LiveRoot];
}

/// Unrestricted database access for recursive semantic queries.
#[salsa::db]
pub trait Db: SourceDb {}

#[salsa::tracked(returns(ref))]
pub(crate) fn live_roots_query(db: &dyn SourceDb) -> Vec<LiveRoot> {
    let mut roots: Vec<LiveRoot> = db
        .workspace_roots()
        .roots(db)
        .iter()
        .map(|&r| LiveRoot::Workspace(r))
        .collect();

    roots.extend(
        db.library_roots()
            .roots(db)
            .iter()
            .map(|&r| LiveRoot::Library(r)),
    );

    roots.push(LiveRoot::Orphan(db.orphan_root()));
    roots
}

/// Files reachable from workspace roots and orphan buffers, plus library
/// files that belong to an actual workspace dependency (see
/// [`crate::all_package_dependencies`]).
///
/// Same as [`all_known_files`] but excluding library packages that aren't a
/// dependency (direct or indirect) of the workspace.
#[salsa::tracked(returns(ref))]
pub fn all_used_files(db: &dyn Db) -> Vec<File> {
    let dependencies: FxHashSet<Package> = crate::workspace::all_package_dependencies(db)
        .iter()
        .copied()
        .collect();

    let mut seen = FxHashSet::default();
    let mut files = Vec::new();

    for &root in db.live_roots() {
        match root {
            LiveRoot::Workspace(root) => push_root_files(db, &mut files, &mut seen, root, None),
            LiveRoot::Library(root) => {
                push_root_files(db, &mut files, &mut seen, root, Some(&dependencies))
            },
            LiveRoot::Orphan(orphan) => {
                for &file in orphan.files(db) {
                    if seen.insert(file) {
                        files.push(file);
                    }
                }
            },
        }
    }

    files
}

/// All files known to the database, in stable order (workspace, library, orphan).
///
/// Note that this also contains files from all installed packages, _including
/// those that are not dependencies of the workspace_. Only use this for very
/// wide searches. LSP functionality should generally not depend on
/// non-dependencies, prefer [`all_used_files()`] instead.
#[salsa::tracked(returns(ref))]
pub fn all_known_files(db: &dyn SourceDb) -> Vec<File> {
    let mut seen = FxHashSet::default();
    let mut files = Vec::new();

    for &root in db.live_roots() {
        match root {
            LiveRoot::Workspace(root) | LiveRoot::Library(root) => {
                push_root_files(db, &mut files, &mut seen, root, None)
            },
            LiveRoot::Orphan(orphan) => {
                for &file in orphan.files(db) {
                    if seen.insert(file) {
                        files.push(file);
                    }
                }
            },
        }
    }

    files
}

/// Pushes `root`'s scripts and package files into `files`, skipping ones
/// already in `seen`. When `dependencies` is `Some`, packages not in that set
/// are skipped entirely.
fn push_root_files(
    db: &dyn SourceDb,
    files: &mut Vec<File>,
    seen: &mut FxHashSet<File>,
    root: Root,
    dependencies: Option<&FxHashSet<Package>>,
) {
    let root_files = root.scripts(db).iter().chain(
        root.packages(db)
            .iter()
            .filter(|&&pkg| dependencies.is_none_or(|deps| deps.contains(&pkg)))
            .flat_map(|&pkg| pkg.files(db).iter().chain(pkg.scripts(db))),
    );
    for &file in root_files {
        if seen.insert(file) {
            files.push(file);
        }
    }
}

/// Files eligible for the workspace symbol index: workspace-root scripts and
/// package files, plus orphan editor buffers. Library roots are excluded, so
/// installed package symbols don't leak into e.g. workspace symbols.
#[salsa::tracked(returns(ref))]
pub fn workspace_files(db: &dyn SourceDb) -> Vec<File> {
    let mut files: Vec<File> = Vec::new();

    for &root in db.live_roots() {
        match root {
            LiveRoot::Workspace(r) => collect_root_files(db, &mut files, r),
            LiveRoot::Library(_) => {},
            LiveRoot::Orphan(orphan) => files.extend(orphan.files(db).iter().copied()),
        }
    }

    files
}

/// The scripts held directly by workspace roots, in root order.
/// Like [`workspace_files`] but without package files.
#[salsa::tracked(returns(ref))]
pub(crate) fn workspace_scripts(db: &dyn SourceDb) -> Vec<File> {
    db.workspace_roots()
        .roots(db)
        .iter()
        .flat_map(|root| root.scripts(db).iter().copied())
        .collect()
}

/// Every file owned by a workspace root, including package files. Orphan
/// buffers are excluded because directory loading is rooted on disk.
#[salsa::tracked(returns(ref))]
pub(crate) fn workspace_root_files(db: &dyn SourceDb) -> Vec<File> {
    let mut files: Vec<File> = Vec::new();
    for &root in db.workspace_roots().roots(db) {
        collect_root_files(db, &mut files, root);
    }
    files
}

fn collect_root_files(db: &dyn SourceDb, files: &mut Vec<File>, r: Root) {
    let owned = |f: File| root_by_file(db, f) == Some(r);
    files.extend(r.scripts(db).iter().copied().filter(|&f| owned(f)));

    for &pkg in r.packages(db) {
        let pkg_files = pkg.files(db).iter().chain(pkg.scripts(db));
        files.extend(pkg_files.copied().filter(|&f| owned(f)));
    }
}

/// Implementation of [`SourceDb::file_by_path`]. Walks the per-root indices.
///
/// Not itself salsa-tracked (its `&FilePath` argument isn't a salsa
/// entity), but every step is: each [`root_path_index`] call returns a
/// cached map, so adding a file to one root invalidates only that
/// root's index.
pub(crate) fn file_by_path_query(db: &dyn SourceDb, path: &FilePath) -> Option<File> {
    for &root in db.live_roots() {
        let hit = match root {
            LiveRoot::Workspace(r) | LiveRoot::Library(r) => {
                root_path_index(db, r).get(path).copied()
            },
            LiveRoot::Orphan(_) => orphan_path_index(db).get(path).copied(),
        };
        if hit.is_some() {
            return hit;
        }
    }
    None
}

/// Implementation of [`SourceDb::package_by_name`]. Same shape as
/// [`file_by_path_query`]; orphan has no packages, so it contributes
/// nothing to the walk.
pub(crate) fn package_by_name_query(db: &dyn SourceDb, name: &str) -> Option<Package> {
    for &root in db.live_roots() {
        if let LiveRoot::Workspace(r) | LiveRoot::Library(r) = root {
            if let Some(&pkg) = root_package_index(db, r).get(name) {
                return Some(pkg);
            }
        }
    }
    None
}

/// Implementation of [`SourceDb::root_by_package`]. Walks all live roots looking for
/// `pkg` in their `packages` vec, picking the longest-path root on ties.
pub(crate) fn root_by_package_query(db: &dyn SourceDb, pkg: Package) -> Option<Root> {
    let mut best: Option<(Root, usize)> = None;
    for &root in db.live_roots() {
        let (LiveRoot::Workspace(r) | LiveRoot::Library(r)) = root else {
            continue;
        };
        if r.packages(db).contains(&pkg) {
            let depth = root_depth(db, r);
            if best.is_none_or(|(_, d)| depth > d) {
                best = Some((r, depth));
            }
        }
    }
    best.map(|(root, _)| root)
}

/// The live root that owns `file`: among the workspace and library roots
/// whose scanned set reaches `file`, the one with the longest path.
///
/// The file-level analogue of [`root_by_package`], used by [`File::root`].
/// Nested roots overlap on disk, so a file reachable from several roots is
/// owned by the deepest one, matching the package tiebreak. Ownership is keyed
/// on reachability (the file is in the root's [`root_path_index`]), not bare
/// path prefix, so the owner always actually contains the file. A bare path
/// prefix would name a freshly-added but not-yet-scanned nested root that
/// doesn't contain the file yet.
///
/// Returns `None` for orphan files (they live in no workspace or library
/// root). [`File::root`] handles that case with a path-prefix fallback.
pub(crate) fn root_by_file(db: &dyn SourceDb, file: File) -> Option<Root> {
    let mut best: Option<(Root, usize)> = None;

    let path = file.path(db);
    for &root in db.live_roots() {
        let (LiveRoot::Workspace(r) | LiveRoot::Library(r)) = root else {
            continue;
        };
        if root_path_index(db, r).contains_key(path) {
            let depth = root_depth(db, r);
            if best.is_none_or(|(_, d)| depth > d) {
                best = Some((r, depth));
            }
        }
    }

    best.map(|(root, _)| root)
}

/// Number of path segments in a root's URL. Used as the tiebreaker by
/// [`root_by_package_query`] and [`root_by_file`] when nested roots both
/// claim the same package or file.
///
/// Counts URL segments directly rather than going through `to_file_path()`.
/// `to_file_path()` errors on Windows for non-OS-style URLs (no drive
/// letter), which would silently collapse all depths to zero and degrade
/// the tiebreaker into "first found wins". Depth is a structural property
/// of the URL hierarchy, so the URL itself is the right source.
fn root_depth(db: &dyn SourceDb, root: Root) -> usize {
    root.path(db)
        .to_url()
        .path_segments()
        .map(|s| s.filter(|seg| !seg.is_empty()).count())
        .unwrap_or(0)
}

/// Per-root URL -> File index. Salsa caches one map per `Root`;
/// reads `root.scripts`, `root.packages`, each `pkg.files`, and each
/// `pkg.scripts` reachable from this root. Adding or removing a file
/// in *this* root invalidates this entry; other roots stay cached.
#[salsa::tracked(returns(ref))]
fn root_path_index(db: &dyn SourceDb, root: Root) -> FxHashMap<FilePath, File> {
    let mut map = FxHashMap::default();
    for &file in root.scripts(db) {
        map.insert(file.path(db).clone(), file);
    }
    for &pkg in root.packages(db) {
        for &file in pkg.files(db) {
            map.insert(file.path(db).clone(), file);
        }
        for &file in pkg.scripts(db) {
            map.insert(file.path(db).clone(), file);
        }
    }
    map
}

/// Orphan URL -> File index. Reads only `orphan_root().files`.
#[salsa::tracked(returns(ref))]
fn orphan_path_index(db: &dyn SourceDb) -> FxHashMap<FilePath, File> {
    let mut map = FxHashMap::default();
    for &file in db.orphan_root().files(db) {
        map.insert(file.path(db).clone(), file);
    }
    map
}

/// Per-root name -> Package index. Same granularity as
/// [`root_path_index`].
#[salsa::tracked(returns(ref))]
fn root_package_index(db: &dyn SourceDb, root: Root) -> FxHashMap<String, Package> {
    let mut map = FxHashMap::default();
    for &pkg in root.packages(db) {
        map.insert(pkg.name(db).clone(), pkg);
    }
    map
}

/// Unchanged notebook membership stops cell-index invalidation from propagating
/// to a file's downstream queries, such as [`File::cross_file_layers()`]. Salsa
/// backdates this query when its result is unchanged. Cell reorders still
/// invalidate [`crate::load_context`] through its direct read of
/// [`Notebook::cells()`].
#[salsa::tracked(returns(copy))]
pub(crate) fn notebook_by_cell(db: &dyn SourceDb, file: File) -> Option<Notebook> {
    notebook_cell_index(db).get(&file).copied()
}

/// Invalidated when the open-notebook list or any open notebook's cell list
/// changes. Cell text edits do not invalidate this index because it reads
/// membership, not source text.
#[salsa::tracked(returns(ref))]
fn notebook_cell_index(db: &dyn SourceDb) -> FxHashMap<File, Notebook> {
    let mut map = FxHashMap::default();
    for &notebook in db.open_notebooks().notebooks(db) {
        for &cell in notebook.cells(db) {
            map.insert(cell, notebook);
        }
    }
    map
}
