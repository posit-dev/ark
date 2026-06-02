use crate::Db;
use crate::Definition;
use crate::Name;
use crate::Package;

/// Visibility filter for [`Package::resolve`].
///
/// Mirrors R's `::` vs `:::` distinction. `Exported` requires `name` to
/// appear in the package's NAMESPACE `export()` directives. `Internal`
/// returns any top-level binding the package's files define, exported
/// or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PackageVisibility {
    Exported,
    Internal,
}

#[salsa::tracked]
impl<'db> Package {
    /// Resolve `name` against this package's top-level bindings.
    ///
    /// - `Exported` (R's `pkg::name`) gates on the package's NAMESPACE `export()`
    ///   directives: an internal binding is invisible even if it exists.
    /// - `Internal` (R's `pkg:::name`) returns any top-level binding regardless
    ///   of NAMESPACE.
    ///
    /// Iterates [`Package::files`] and aggregates each file's
    /// [`File::resolve_export`] for `name`.
    ///
    /// Returns a `Vec` because a package's namespace can carry more than one
    /// binding per name. A common pattern is a top-level stub overridden from
    /// an `.onLoad` hook via `<<-`:
    ///
    /// ```r
    /// # R/foo.R
    /// foo <- function() stop("not loaded yet")
    ///
    /// # R/zzz.R
    /// .onLoad <- function(libname, pkgname) {
    ///   foo <<- function() "real implementation"
    /// }
    /// ```
    ///
    /// Both bindings live in the package namespace; both come back as
    /// candidates. Conditional fan-out within a single file (e.g.
    /// `if cond x <- 1 else x <- 2`) surfaces here the same way.
    ///
    /// Returns an empty Vec when the name isn't bound anywhere in the package,
    /// or when `Exported` is requested for a name absent from
    /// `namespace.exports`.
    #[salsa::tracked]
    pub fn resolve(
        self,
        db: &'db dyn Db,
        name: Name<'db>,
        visibility: PackageVisibility,
    ) -> Vec<Definition<'db>> {
        if visibility == PackageVisibility::Exported &&
            !self
                .namespace(db)
                .exports
                .contains_str(name.text(db).as_str())
        {
            return Vec::new();
        }

        let mut results = Vec::new();
        for &file in self.files(db) {
            results.extend(file.resolve_export(db, name));
        }

        results
    }
}
