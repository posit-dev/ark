use crate::SourceGraph;
use crate::WorkspaceRoots;

/// Salsa Database trait.
///
/// Queries take a `dyn Db` rather than the concrete database owned by
/// the LSP layer.
#[salsa::db]
pub trait Db: salsa::Database {
    /// The workspace's source graph. Each concrete database holds one
    /// salsa input handle for the lifetime of the database. The
    /// recommended implementation lazily allocates it on first access
    /// via `Arc<OnceLock<SourceGraph>>`.
    fn source_graph(&self) -> SourceGraph;

    /// Workspace folders opened by the editor.
    ///
    /// Bumps to each `Root`'s revision are the salsa-observable signal
    /// for "the file set under this root changed."
    fn workspace_roots(&self) -> WorkspaceRoots;
}
