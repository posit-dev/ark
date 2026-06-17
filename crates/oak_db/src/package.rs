use std::fs;
use std::io;

use aether_path::FilePath;
use oak_package_metadata::description::Description;
use oak_package_metadata::namespace::Namespace;
use stdext::result::ResultExt;

use crate::file_revision::report_untracked_if_zero;
use crate::Db;
use crate::File;
use crate::FileRevision;

#[salsa::input(debug)]
pub struct Package {
    /// URL of the package's `DESCRIPTION` file. Stable identity across
    /// rescans and workspace / library churn: scanners look up an
    /// existing `Package` by this URL before creating a new one. Two
    /// packages with the same `Package:` name can coexist on disk and the
    /// URL distinguishes them.
    ///
    /// The package's owning [`Root`] is not stored as a field. It is
    /// derived from live-graph containment via [`Db::root_by_package`]: a
    /// package belongs to whichever `Root.packages` currently holds it.
    /// Workspace-vs-library is then `root.kind(db)`.
    #[returns(ref)]
    pub description_path: FilePath,
    // TODO(salsa): Expose a tracked `name_interned(db) -> Name<'db>`
    // method so `db.package_by_name()` and other lookups key on the
    // interned id rather than the string. Can't store `Name<'db>` on
    // `Package` directly because salsa inputs are lifetime-free.
    #[returns(ref)]
    pub name: String,
    /// Mtime of the package's `DESCRIPTION` file, or [`FileRevision::zero`]
    /// when it can't be stat'd. Drives the lazy [`Package::description`]
    /// query (and the `version` / `collation` derivations on top of it), the
    /// same way `namespace_revision` drives [`Package::namespace`]. We stat
    /// `DESCRIPTION` at scan time but parse it only on demand.
    pub description_revision: FileRevision,
    /// Mtime of the package's `NAMESPACE` file, or [`FileRevision::zero`]
    /// when it can't be stat'd. The lazy [`Package::namespace`] query reads
    /// it so a watcher that bumps it on a `NAMESPACE` change forces the next
    /// parse to re-read disk, exactly like [`File::revision`] drives
    /// [`File::source_text`]. We don't read or parse `NAMESPACE` at scan
    /// time, only stat it, so installed packages the user never imports cost
    /// nothing beyond the stat.
    pub namespace_revision: FileRevision,
    /// In-memory `NAMESPACE`, checked by [`Package::namespace`] before it
    /// touches disk. Mirrors [`File::source_text_override`]: `None` means
    /// "read from disk". The scanners always leave it `None`. It's the
    /// injection point unit tests use to supply a namespace for a synthetic
    /// path with no file behind it, and the natural hook for an editor
    /// editing a package's `NAMESPACE` live.
    #[returns(ref)]
    pub namespace_override: Option<Namespace>,
    /// R source files belonging to this package (the `R/*.R` files), in
    /// R's load order. When DESCRIPTION's `Collate:` directive is
    /// present, this is exactly the files it lists, in that order;
    /// files in `R/` not listed are excluded (matching R's loader,
    /// Writing R Extensions §1.1.1). When `Collate:` is absent, files
    /// are in case-insensitive alphabetical order. TODO(diagnostics):
    /// Lint files missing from collation.
    ///
    /// Per-package granularity: adding or removing a file in one
    /// package doesn't invalidate tracked queries reading another
    /// package's files.
    ///
    /// **Placement invariant.** A file present here must have
    /// `package(db) == Some(self)`, and a file with
    /// `package == Some(self)` must live here or in [`Self::scripts`].
    /// Call this setter only through `oak_scan`'s helpers, which keep
    /// the back-pointer and the container in sync.
    #[returns(ref)]
    pub files: Vec<File>,
    /// Other R files inside the package directory that aren't part of the
    /// loadable namespace: `tests/`, `inst/`, `data-raw/`, etc. These get LSP
    /// analysis (parse, semantic index) but aren't loaded with the package, so
    /// name resolution treats them as standalone scripts that just happen to
    /// live next to the package's code.
    ///
    /// **Placement invariant.** Same as [`Self::files`]: backpointer
    /// stays `Some(self)`, file lives in one of the two containers.
    #[returns(ref)]
    pub scripts: Vec<File>,
}

#[salsa::tracked]
impl Package {
    /// The package's parsed `NAMESPACE`.
    ///
    /// Returns the in-memory override if one is set
    /// (`namespace_override`), otherwise reads `NAMESPACE` from disk and
    /// parses it. Lazy and tracked so we only pay the read and the R-parse
    /// for packages whose imports actually get resolved. Parsing every
    /// installed package's `NAMESPACE` eagerly would dominate the cost of
    /// scanning a library with hundreds of packages, so we defer it (#1265).
    ///
    /// A missing or unparseable `NAMESPACE` yields an empty `Namespace`.
    #[salsa::tracked(returns(ref))]
    pub fn namespace(self, db: &dyn Db) -> Namespace {
        if let Some(namespace) = self.namespace_override(db) {
            return namespace.clone();
        }

        // Depend on `namespace_revision()` so a bump forces a re-read
        report_untracked_if_zero(db, self.namespace_revision(db));

        let Some(dir) = self
            .description_path(db)
            .as_path()
            .and_then(|path| path.parent())
        else {
            return Namespace::default();
        };

        let namespace_path = dir.join("NAMESPACE");
        match fs::read_to_string(namespace_path.as_std_path()) {
            Ok(text) => Namespace::parse(&text).log_err().unwrap_or_default(),
            // A package needn't ship a `NAMESPACE`, so absence is the normal
            // case and stays quiet. A file that exists but can't be read is
            // logged so the failure isn't silently read as "no namespace".
            Err(err) if err.kind() == io::ErrorKind::NotFound => Namespace::default(),
            Err(err) => {
                log::error!("Failed to read `{namespace_path}`: {err:?}");
                Namespace::default()
            },
        }
    }

    /// The package's `Version:`, parsed lazily from `DESCRIPTION`. `None`
    /// when the file is missing or has no version.
    ///
    /// A narrow query over [`Package::description`]: editing `DESCRIPTION`
    /// without changing `Version:` backdates here, so downstream isn't
    /// disturbed.
    #[salsa::tracked(returns(ref))]
    pub fn version(self, db: &dyn Db) -> Option<String> {
        self.description(db)
            .as_ref()
            .map(|description| description.version.clone())
    }

    /// The basename order from `DESCRIPTION`'s `Collate:` field, parsed
    /// lazily. `None` when the field (or the file) is absent. Narrow query
    /// over [`Package::description`], same backdating story as
    /// [`Package::version`].
    #[salsa::tracked(returns(ref))]
    pub fn collation(self, db: &dyn Db) -> Option<Vec<String>> {
        self.description(db)
            .as_ref()
            .and_then(|description| description.collate())
    }

    /// The package's parsed `DESCRIPTION`, or `None` when it's missing or
    /// unparseable.
    ///
    /// Lazy and tracked, keyed on `description_revision`. Reading is deferred
    /// so the library scanner can register an installed package without
    /// touching `DESCRIPTION` at all (it takes the name from the directory).
    ///
    /// `Description` is `PartialEq`, so salsa backdates this query when a
    /// `description_revision` bump turns out to leave the parsed content
    /// unchanged. The narrow `version` / `collation` queries on top firewall
    /// the other direction: a real `DESCRIPTION` edit that doesn't touch
    /// `Version:` / `Collate:` re-runs this query but backdates there.
    #[salsa::tracked(returns(ref))]
    pub(crate) fn description(self, db: &dyn Db) -> Option<Description> {
        // Depend on `description_revision()` so a bump forces a re-read
        report_untracked_if_zero(db, self.description_revision(db));

        let path = self.description_path(db).as_path()?;
        match fs::read_to_string(path.as_std_path()) {
            Ok(text) => Description::parse(&text).log_err(),
            // A missing `DESCRIPTION` is the normal "gone after a rescan" case
            // and stays quiet. A file that exists but can't be read is logged
            // rather than silently treated as absent.
            Err(err) if err.kind() == io::ErrorKind::NotFound => None,
            Err(err) => {
                log::error!("Failed to read `{path}`: {err:?}");
                None
            },
        }
    }
}
