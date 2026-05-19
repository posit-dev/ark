use crate::SourceGraph;

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
}
