use std::fs;
use std::io;

use oak_package_metadata::description::Description;
use oak_package_metadata::namespace::Namespace;
use stdext::result::ResultExt;

use crate::Db;
use crate::Package;

#[salsa::tracked]
impl Package {
    /// The package's parsed `NAMESPACE`.
    ///
    /// Returns the in-memory override if one is set
    /// (`namespace_override`), otherwise reads `NAMESPACE` from disk and
    /// parses it. Lazy and tracked so we only pay the read and the R-parse
    /// for packages whose imports actually get resolved. Parsing every
    /// installed package's `NAMESPACE` eagerly would dominate the cost of
    /// scanning a library with hundreds of packages, so we defer it.
    ///
    /// A missing or unparseable `NAMESPACE` yields an empty `Namespace`.
    #[salsa::tracked(returns(ref))]
    pub fn namespace(self, db: &dyn Db) -> Namespace {
        if let Some(namespace) = self.namespace_override(db) {
            return namespace.clone();
        }

        // Reading `namespace_revision` makes this memo depend on it even though
        // the value isn't used here: bumping the revision is what forces a
        // re-read.
        let _ = self.namespace_revision(db);

        let Some(dir) = self
            .description_path(db)
            .as_path()
            .and_then(|path| path.parent())
        else {
            return Namespace::default();
        };

        let namespace_path = dir.join("NAMESPACE");
        let text = match fs::read_to_string(namespace_path.as_std_path()) {
            Ok(text) => text,
            // A package needn't ship a `NAMESPACE`, so absence is the normal
            // case and stays quiet. A file that exists but can't be read is
            // logged so the failure isn't silently read as "no namespace".
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Namespace::default(),
            Err(err) => {
                log::error!("Failed to read `{namespace_path}`: {err:?}");
                return Namespace::default();
            },
        };

        Namespace::parse(&text).log_err().unwrap_or_default()
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
        let _ = self.description_revision(db);

        let path = self.description_path(db).as_path()?;
        let text = match fs::read_to_string(path.as_std_path()) {
            Ok(text) => text,
            // A missing `DESCRIPTION` is the normal "gone after a rescan" case
            // and stays quiet. A file that exists but can't be read is logged
            // rather than silently treated as absent.
            Err(err) if err.kind() == io::ErrorKind::NotFound => return None,
            Err(err) => {
                log::error!("Failed to read `{path}`: {err:?}");
                return None;
            },
        };
        Description::parse(&text).log_err()
    }
}
