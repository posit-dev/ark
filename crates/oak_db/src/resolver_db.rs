//! Restricts database access during source and effect resolution.
//!
//! [`FoundationDb`] exposes operations that do not re-enter semantic analysis.
//! [`ResolverDb`] contains that wrapper and adds three recursive queries with
//! `cycle_result` handlers. Resolution can borrow the narrower capabilities
//! through [`ResolverDb::foundation()`] without exposing the underlying database.
//!
//! Both wrappers keep database access private to this module. Their forwarding
//! methods are trusted code, so changes require a transitive dependency review.
//! Recovery helpers must accept only [`FoundationDb`], not [`ResolverDb`].

use aether_path::FilePath;
use oak_package_metadata::namespace::Namespace;
use oak_semantic::effects::DirWalk;
use rustc_hash::FxHashMap;

use crate::file_imports::CollationView;
use crate::file_imports::CrossFileLayers;
use crate::imports::source_dir_scripts;
use crate::Db;
use crate::File;
use crate::FileExports;
use crate::Package;
use crate::Root;
use crate::RootKind;

#[derive(Clone, Copy)]
pub(crate) struct ResolverDb<'db> {
    foundation: FoundationDb<'db>,
}

impl<'db> ResolverDb<'db> {
    pub(crate) fn new(db: &'db dyn Db) -> Self {
        Self {
            foundation: FoundationDb::new(db),
        }
    }

    pub(crate) fn foundation(self) -> FoundationDb<'db> {
        self.foundation
    }

    // These queries can re-enter semantic analysis and have recovery handlers.

    pub(crate) fn exports(self, file: File) -> &'db FileExports {
        file.exports(self.foundation.db)
    }

    /// Package names from `library()` calls at `file`'s own top level.
    ///
    /// Returning text keeps interned [`Name`](crate::Name) handles and their
    /// database access inside this wrapper.
    pub(crate) fn attached_package_names(self, file: File) -> Vec<String> {
        file.attached_packages(self.foundation.db)
            .iter()
            .map(|name| name.text(self.foundation.db).to_string())
            .collect()
    }

    pub(crate) fn cross_file_layers(self, file: File, view: CollationView) -> &'db CrossFileLayers {
        file.cross_file_layers(self.foundation.db, view)
    }
}

/// Inputs, per-root indices, and package metadata available without recursive
/// semantic queries. The raw database has no accessor, so callers cannot widen
/// this wrapper back to [`ResolverDb`] or `&dyn Db`.
#[derive(Clone, Copy)]
pub(crate) struct FoundationDb<'db> {
    db: &'db dyn Db,
}

impl<'db> FoundationDb<'db> {
    pub(crate) fn new(db: &'db dyn Db) -> Self {
        Self { db }
    }

    pub(crate) fn file_by_path(self, path: &FilePath) -> Option<File> {
        Db::file_by_path(self.db, path)
    }

    pub(crate) fn package_by_name(self, name: &str) -> Option<Package> {
        Db::package_by_name(self.db, name)
    }

    pub(crate) fn file_path(self, file: File) -> &'db FilePath {
        file.path(self.db)
    }

    pub(crate) fn file_root(self, file: File) -> Option<Root> {
        file.root(self.db)
    }

    pub(crate) fn root_kind(self, root: Root) -> RootKind {
        root.kind(self.db)
    }

    pub(crate) fn root_path(self, root: Root) -> &'db FilePath {
        root.path(self.db)
    }

    pub(crate) fn source_dir_scripts(self, file: File, path: String, walk: DirWalk) -> &'db [File] {
        source_dir_scripts(self.db, file, path, walk)
    }

    pub(crate) fn package_name(self, package: Package) -> &'db str {
        package.name(self.db).as_str()
    }

    pub(crate) fn package_namespace(self, package: Package) -> &'db Namespace {
        package.namespace(self.db)
    }

    pub(crate) fn package_imported_from(self, package: Package) -> &'db FxHashMap<String, String> {
        package.imported_from(self.db)
    }
}
