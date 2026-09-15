//! These probes use the resolver's database field so weakening its type
//! also changes which calls compile.
//!
//! The boundary tests compile this crate with `resolver_boundary = "probe"`
//! to check that recursive queries and access to the underlying database are
//! rejected. A separate build with `resolver_boundary = "control"` checks
//! that `allowed_calls()` compiles, so unrelated errors
//! cannot make a rejected call look like successful enforcement.

#[cfg(resolver_boundary = "control")]
use aether_path::FilePath;

use super::SalsaImportsResolver;
#[cfg(any(resolver_boundary = "probe", resolver_boundary = "control"))]
use crate::file_imports::CollationView;
#[cfg(resolver_boundary = "probe")]
use crate::Db;

impl<'db> SalsaImportsResolver<'db> {
    #[cfg(resolver_boundary = "probe")]
    fn calls_semantic_index_directly(&self) {
        let _ = self.file.semantic_index(self.db);
    }

    #[cfg(resolver_boundary = "probe")]
    fn calls_diagnostics_directly(&self) {
        let _ = self.file.diagnostics(self.db);
    }

    #[cfg(resolver_boundary = "probe")]
    fn calls_imports_directly(&self) {
        let _ = self.file.imports(self.db);
    }

    #[cfg(resolver_boundary = "probe")]
    fn escapes_to_dyn_db(&self) {
        let _: &dyn Db = self.db;
    }

    #[cfg(resolver_boundary = "probe")]
    fn escapes_through_private_field(&self) {
        let _: &dyn Db = self.db.db;
    }

    #[cfg(resolver_boundary = "probe")]
    fn sources_call_exports(&self) {
        let _ = self.file.exports(self.db.as_source_db());
    }

    #[cfg(resolver_boundary = "probe")]
    fn sources_call_attached_packages(&self) {
        let _ = self.file.attached_packages(self.db.as_source_db());
    }

    #[cfg(resolver_boundary = "probe")]
    fn sources_call_cross_file_layers(&self) {
        let _ = self
            .file
            .cross_file_layers(self.db.as_source_db(), CollationView::Eager);
    }

    #[cfg(resolver_boundary = "probe")]
    fn sources_escape_to_dyn_db(&self) {
        let _: &dyn Db = self.db.as_source_db();
    }

    #[cfg(resolver_boundary = "control")]
    fn allowed_calls(&self) {
        let _ = self.db.exports(self.file);
        let _ = self.db.attached_package_names(self.file);
        let _ = self.db.cross_file_layers(self.file, CollationView::Eager);
        let _ = self.file.path(self.db.as_source_db());
        let _ = self.file.parse(self.db.as_source_db());
    }

    #[cfg(resolver_boundary = "control")]
    fn borrowed_path(&self) -> &'db FilePath {
        self.file.path(self.db.as_source_db())
    }
}
