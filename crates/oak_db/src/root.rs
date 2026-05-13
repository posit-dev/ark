use aether_url::UrlId;

use crate::Db;

/// Salsa-tracked root directory.
///
/// `revision` bumps when files are added, removed, or renamed within this
/// directory. Contents-only edits don't bump. Per-file content changes are
/// observed via `File`'s salsa input fields.
///
/// Workspace folders, workspace packages, and installed-package libpaths are
/// tracked as `Root`s. Workspace folders live in [`crate::WorkspaceRoots`].
/// Workspace packages reference their `Root` via `PackageOrigin::Workspace {
/// root }`.
#[salsa::input(debug)]
pub struct Root {
    #[returns(ref)]
    pub path: UrlId,
    pub kind: RootKind,
    pub revision: u64,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum RootKind {
    Workspace,
}

/// Workspace root that contains `url`, longest-prefix among nested
/// roots.
///
/// Returns `None` for non-`file:` URLs and for URLs that don't lie under
/// any workspace folder. Walks [`crate::WorkspaceRoots`] linearly.
pub fn url_to_root(db: &dyn Db, url: &UrlId) -> Option<Root> {
    let path = url.to_file_path()?;
    db.workspace_roots()
        .roots(db)
        .iter()
        .filter_map(|root| {
            let root_path = root.path(db).to_file_path()?;
            path.starts_with(&root_path).then_some((root_path, *root))
        })
        .max_by_key(|(p, _)| p.components().count())
        .map(|(_, r)| r)
}
