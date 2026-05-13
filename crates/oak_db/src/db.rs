use crate::Files;
use crate::SourceGraph;
use crate::WorkspaceRoots;

/// Salsa Database trait.
///
/// Queries take a `dyn Db` rather than the concrete database owned by
/// the LSP layer.
#[salsa::db]
pub trait Db: salsa::Database {
    /// Source graph of script and package nodes.
    fn source_graph(&self) -> SourceGraph {
        SourceGraph::get(self)
    }

    /// Workspace folders opened by the editor.
    ///
    /// Bumps to each `Root`'s revision are the salsa-observable signal
    /// for "the file set under this root changed."
    fn workspace_roots(&self) -> WorkspaceRoots {
        WorkspaceRoots::get(self)
    }

    /// URL-keyed `File` interner.
    fn files(&self) -> &Files;
}
