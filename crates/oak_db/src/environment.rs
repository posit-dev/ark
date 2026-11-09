//! File-to-environment lookup and workspace anchoring for `source()` paths.
//!
//! [`workspace_anchor()`] selects the nearest enclosing environment directory
//! within the file's workspace root, or the root itself. Both
//! [`File::environment()`] and [`File::anchor_dir()`] use this directory. If it has no live environment, environment lookup falls
//! back to the root's environment and then the default, while `source()` paths
//! remain anchored at the selected directory. Notebook cells use their notebook's
//! path for environment lookup, but have no anchor for relative `source()` paths.

use aether_path::FilePath;
use camino::Utf8Path;
use rustc_hash::FxHashMap;

use crate::db::notebook_by_cell;
use crate::file::root_by_path;
use crate::Environment;
use crate::File;
use crate::Root;
use crate::RootKind;
use crate::SourceDb;

/// A file's workspace root and its nearest enclosing environment directory,
/// if the root's last scan found one.
pub(crate) struct WorkspaceAnchor<'db> {
    pub(crate) root: Root,
    pub(crate) environment_dir: Option<&'db FilePath>,
}

impl<'db> WorkspaceAnchor<'db> {
    /// The nearest enclosing environment directory, else the root.
    pub(crate) fn dir(&self, db: &'db dyn SourceDb) -> &'db FilePath {
        self.environment_dir.unwrap_or_else(|| self.root.path(db))
    }
}

/// Select the workspace directory that anchors a file's relative `source()`
/// paths. Return `None` for virtual documents and files whose [`File::root()`]
/// is absent or is a library root, even if that library is inside a workspace.
///
/// The root is [`File::root()`], so a buffer opened before the first scan
/// still finds its workspace root by path. Its environment directories come
/// from that root's last scan.
pub(crate) fn workspace_anchor(db: &dyn SourceDb, file: File) -> Option<WorkspaceAnchor<'_>> {
    let file_path = file.path(db).as_path()?;
    let root = file
        .root(db)
        .filter(|root| root.kind(db) == RootKind::Workspace)?;
    Some(anchor_in_root(db, root, file_path))
}

/// Select the environment lookup directory from the notebook's path because
/// the cell's virtual URI has no [`workspace_anchor()`].
///
/// [`File::anchor_dir()`] does not use this anchor, so
/// relative `source()` paths in cells have no anchor.
fn notebook_anchor(db: &dyn SourceDb, file: File) -> Option<WorkspaceAnchor<'_>> {
    let notebook = notebook_by_cell(db, file)?;
    let notebook_path = notebook.path(db);
    let root = root_by_path(db, notebook_path)?;
    Some(anchor_in_root(db, root, notebook_path.as_path()?))
}

fn anchor_in_root<'db>(db: &'db dyn SourceDb, root: Root, path: &Utf8Path) -> WorkspaceAnchor<'db> {
    // `environment_dirs` is sorted, so the last enclosing directory is the
    // nearest one.
    let environment_dir = root
        .environment_dirs(db)
        .iter()
        .rfind(|dir| dir.as_path().is_some_and(|dir| path.starts_with(dir)));

    WorkspaceAnchor {
        root,
        environment_dir,
    }
}

/// If the workspace anchor has no live environment, use the root's
/// environment, then the default. Without workspace environments, every file
/// maps to the default. Notebook cells select the anchor from their notebook's
/// path, not their virtual URI.
pub(crate) fn file_environment(db: &dyn SourceDb, file: File) -> Environment {
    let anchor = workspace_anchor(db, file).or_else(|| notebook_anchor(db, file));
    let Some(anchor) = anchor else {
        return db.default_environment();
    };
    let environments = environments_by_dir(db);
    environments
        .get(anchor.dir(db))
        .or_else(|| environments.get(anchor.root.path(db)))
        .copied()
        .unwrap_or_else(|| db.default_environment())
}

/// Index live workspace environments by directory. Changes to the environment
/// list or an environment's `dir` invalidate this query. Changes to `status`
/// or `last_error` do not.
#[salsa::tracked(returns(ref))]
fn environments_by_dir(db: &dyn SourceDb) -> FxHashMap<FilePath, Environment> {
    db.workspace_environments()
        .environments(db)
        .iter()
        .filter_map(|&environment| Some((environment.dir(db).clone()?, environment)))
        .collect()
}
