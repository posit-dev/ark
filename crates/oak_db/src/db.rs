/// Salsa Database trait.
///
/// Queries take a `dyn Db` rather than the concrete database owned by
/// the LSP layer.
#[salsa::db]
pub trait Db: salsa::Database {}
