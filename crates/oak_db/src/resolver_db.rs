//! Restricts the recursive queries available during resolution.
//!
//! Source operations use [`SourceDb`] directly. This wrapper exposes only the
//! three recursive queries needed by [`SalsaImportsResolver`], each backed by
//! a `cycle_result` handler. Their bodies still require the full [`Db`].
//!
//! The raw database is private, so callers cannot bypass these forwarding
//! methods. Recovery helpers receive only `&dyn SourceDb`.
//!
//! Forwarding bodies retain unrestricted access; review their transitive
//! dependencies when changing them.
//!
//! [`SalsaImportsResolver`]: crate::imports::SalsaImportsResolver

use crate::file_imports::CollationView;
use crate::file_imports::CrossFileLayers;
use crate::Db;
use crate::File;
use crate::FileExports;
use crate::SourceDb;

#[derive(Clone, Copy)]
pub(crate) struct ResolverDb<'db> {
    db: &'db dyn Db,
}

impl<'db> ResolverDb<'db> {
    pub(crate) fn new(db: &'db dyn Db) -> Self {
        Self { db }
    }

    pub(crate) fn as_source_db(self) -> &'db dyn SourceDb {
        self.db
    }

    pub(crate) fn exports(self, file: File) -> &'db FileExports {
        file.exports(self.db)
    }

    /// Includes only top-level `library()` calls. Returning text keeps interned
    /// [`Name`](crate::Name) handles and their database access inside this wrapper.
    pub(crate) fn attached_package_names(self, file: File) -> Vec<String> {
        file.attached_packages(self.db)
            .iter()
            .map(|name| name.text(self.db).to_string())
            .collect()
    }

    pub(crate) fn cross_file_layers(self, file: File, view: CollationView) -> &'db CrossFileLayers {
        file.cross_file_layers(self.db, view)
    }
}
