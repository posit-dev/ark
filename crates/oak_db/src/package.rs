use std::io;

use aether_path::FilePath;
use oak_package_metadata::description::Description;
use oak_package_metadata::description::Priority;
use oak_package_metadata::index::Index;
use oak_package_metadata::namespace::Namespace;
use regex::Regex;
use regex::RegexSet;
use rustc_hash::FxHashMap;
use salsa::Setter;
use stdext::result::ResultExt;

use crate::file_revision::report_untracked_if_zero;
use crate::File;
use crate::FileRevision;
use crate::RootKind;
use crate::SourceDb;

#[salsa::input(debug)]
pub struct Package {
    /// URL of the package's `DESCRIPTION` file. Stable identity across
    /// rescans and workspace / library churn: scanners look up an
    /// existing `Package` by this URL before creating a new one. Two
    /// packages with the same `Package:` name can coexist on disk and the
    /// URL distinguishes them.
    ///
    /// The package's owning [`Root`] is not stored as a field. It is
    /// derived from live-graph containment via [`SourceDb::root_by_package`]: a
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
    #[returns(copy)]
    pub description_revision: FileRevision,
    /// See [Package::description_revision].
    #[returns(copy)]
    pub namespace_revision: FileRevision,
    /// See [Package::description_revision]. [None] for workspace packages, which never
    /// have an `INDEX`.
    #[returns(copy)]
    pub index_revision: Option<FileRevision>,
    /// In-memory `NAMESPACE`, checked by [`Package::namespace`] before it
    /// touches disk. Mirrors [`File::source_text_override`]: `None` means
    /// "read from disk". The scanners always leave it `None`. It's the
    /// injection point unit tests use to supply a namespace for a synthetic
    /// path with no file behind it, and the natural hook for an editor
    /// editing a package's `NAMESPACE` live.
    #[returns(ref)]
    pub namespace_override: Option<Namespace>,
    /// Private so Salsa's generated setter cannot bypass the operations that
    /// preserve source and generated files in [`Package::files()`]. Salsa's
    /// generated getter and setter inherit the field's visibility.
    ///
    /// Per-package granularity: adding or removing a file in one
    /// package doesn't invalidate tracked queries reading another
    /// package's files.
    #[returns(ref)]
    membership: PackageFiles,
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
    /// Private so writes go through [`Package::set_lazy_data()`], which skips
    /// unchanged values. See [`Package::lazy_data()`].
    #[default]
    #[returns(ref)]
    lazy_data_names: Vec<String>,
    /// The [`Self::description_revision`] that the source files in
    /// [`Self::files`] and [`Self::scripts`] were fetched for, or [None] when
    /// they don't hold fetched sources: workspace packages, and library
    /// packages whose fetch hasn't landed or whose sources were cleared. The
    /// LSP source scheduler fetches whenever this differs from the current
    /// `description_revision`. Generated files don't count: they come from a
    /// separate job.
    ///
    /// The db holds this rather than the scheduler because
    /// [`IdentityMap`](crate::IdentityMap) hands back the same `Package` when
    /// a removed package is reinstalled or a library root is re-added. The
    /// helper that empties the package's lists resets this field, so the
    /// revived package is fetched again without the scheduler being told.
    ///
    /// Written only by `oak_scan`: set by `set_package_sources()`, reset by
    /// every other writer of the source files and `scripts`.
    #[default]
    #[returns(copy)]
    pub sources_revision: Option<FileRevision>,
}

impl Package {
    /// Lazy data object names available through `pkg::name`, sorted and
    /// without duplicates. Names default to empty until the host reads the
    /// installed package's `data/Rdata.rds` with R and supplies them through
    /// [`Package::set_lazy_data()`]. Hosts do not currently supply names for
    /// workspace packages.
    pub fn lazy_data<Db: ?Sized + salsa::Database>(self, db: &Db) -> &[String] {
        self.lazy_data_names(db)
    }

    /// Replaces the lazy data object names, sorting and deduplicating the
    /// input. Skips the write when the normalized names are unchanged,
    /// because even an equal input write advances Salsa's revision.
    pub fn set_lazy_data<Db: ?Sized + salsa::Database>(self, db: &mut Db, mut names: Vec<String>) {
        names.sort();
        names.dedup();
        if self.lazy_data_names(db) != names.as_slice() {
            self.set_lazy_data_names(db).to(names);
        }
    }

    /// Whether the package is installed in a library root, so that its
    /// fetched sources or host-supplied lazy data can still arrive and change
    /// an answer that read it. Answers report only these packages as
    /// dependencies. A workspace package's files are scanned, and an edit to
    /// them invalidates the answer directly; it also shadows an installed
    /// package of the same name ([`SourceDb::package_by_name()`]), so the
    /// installed copy isn't read.
    pub fn is_installed(self, db: &dyn SourceDb) -> bool {
        db.root_by_package(self)
            .is_some_and(|root| root.kind(db) == RootKind::Library)
    }
}

/// File membership operations skip unchanged values because Salsa advances
/// the revision on every input write, even when the value is equal.
///
/// **Placement invariant.** A file in [`Package::files()`] must point back to
/// this package through `File::package()`. Every file with that backpointer
/// must be in this list or [`Package::scripts`]. Call these operations only through
/// `oak_scan`'s helpers, which keep the back-pointer and the container in
/// sync.
impl Package {
    /// Files used to analyze the package's namespace. Source files follow R's
    /// load order, and generated files come after all source files.
    ///
    /// The source files are the package's `R/*.R` files. When `DESCRIPTION`'s
    /// `Collate:` directive is present, they are exactly the files it lists, in
    /// that order. Files in `R/` not listed are excluded, matching R's loader
    /// (Writing R Extensions, section 1.1.1). When `Collate:` is absent, they
    /// are in raw basename byte order, matching R's `LC_COLLATE=C` installation collation.
    /// TODO(diagnostics): Lint files missing from collation.
    ///
    /// Generated files, such as `base`'s primitive stubs, supply bindings with
    /// no package source definition and are not subject to `Collate:`.
    pub fn files<Db: ?Sized + salsa::Database>(self, db: &Db) -> &[File] {
        self.membership(db).files()
    }

    /// Replace the source files, keeping the generated files after them.
    pub fn set_source_files<Db: ?Sized + salsa::Database>(self, db: &mut Db, sources: Vec<File>) {
        if let Some(membership) = self.membership(db).with_sources(sources) {
            self.set_membership(db).to(membership);
        }
    }

    /// Replace the generated files, keeping the source files before them.
    pub fn set_generated_files<Db: ?Sized + salsa::Database>(
        self,
        db: &mut Db,
        generated: Vec<File>,
    ) {
        if let Some(membership) = self.membership(db).with_generated(generated) {
            self.set_membership(db).to(membership);
        }
    }

    /// Append `file` to the source files, unless the package already lists it.
    pub fn add_source_file<Db: ?Sized + salsa::Database>(self, db: &mut Db, file: File) {
        if let Some(membership) = self.membership(db).with_source_added(file) {
            self.set_membership(db).to(membership);
        }
    }

    /// Remove `file` from the source or generated files. Returns whether the
    /// package listed it.
    pub fn remove_file<Db: ?Sized + salsa::Database>(self, db: &mut Db, file: File) -> bool {
        let Some(membership) = self.membership(db).without(file) else {
            return false;
        };
        self.set_membership(db).to(membership);
        true
    }

    /// Remove every source and generated file.
    pub fn clear_files<Db: ?Sized + salsa::Database>(self, db: &mut Db) {
        if !self.membership(db).files().is_empty() {
            self.set_membership(db).to(PackageFiles::default());
        }
    }
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
    pub fn namespace(self, db: &dyn SourceDb) -> Namespace {
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
        match db.read_to_string(&namespace_path) {
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

    /// NAMESPACE `importFrom` entries keyed by imported name, each mapping to
    /// its source package name. Lets the resolution paths look up a name
    /// directly instead of scanning [`Namespace::imports`].
    ///
    /// The value is the source package's name, not a resolved `Package`:
    /// resolving through [`SourceDb::package_by_name`] depends on which roots are
    /// live, and baking that in here would invalidate this index on every
    /// root change. Call sites resolve the name themselves.
    #[salsa::tracked(returns(ref))]
    pub fn imported_from(self, db: &dyn SourceDb) -> FxHashMap<String, String> {
        self.namespace(db)
            .imports
            .iter()
            .map(|import| (import.name.clone(), import.package.clone()))
            .collect()
    }

    /// Cache compiled `exportPattern()` regexes until `NAMESPACE` changes.
    /// Log and skip patterns unsupported by Rust's `regex` crate, including
    /// back-references accepted by R.
    #[salsa::tracked(returns(ref))]
    pub fn export_patterns(self, db: &dyn SourceDb) -> ExportPatterns {
        ExportPatterns::new(&self.namespace(db).export_patterns)
    }

    /// The package's `Version:`, parsed lazily from `DESCRIPTION`. `None`
    /// when the file is missing or has no version.
    ///
    /// A narrow query over [`Package::description`]: editing `DESCRIPTION`
    /// without changing `Version:` backdates here, so downstream isn't
    /// disturbed.
    #[salsa::tracked(returns(ref))]
    pub fn version(self, db: &dyn SourceDb) -> Option<String> {
        self.description(db)
            .as_ref()
            .map(|description| description.version.clone())
    }

    /// The package's `Built:`, parsed lazily from `DESCRIPTION`. `None` when
    /// the file is missing or has no `Built:` field (only installed packages have one).
    ///
    /// Build timestamps change on every install, so backdating probably isn't that
    /// important here, so we don't track this method.
    pub fn built(self, db: &dyn SourceDb) -> Option<String> {
        self.description(db)
            .as_ref()
            .and_then(|description| description.built.clone())
    }

    /// The package's `Priority:`, parsed lazily from `DESCRIPTION`. `None` when the file
    /// is missing or has no `Priority:` field (only base and recommended packages have
    /// one)
    #[salsa::tracked(returns(ref))]
    pub fn priority(self, db: &dyn SourceDb) -> Option<Priority> {
        self.description(db)
            .as_ref()
            .and_then(|description| description.priority.clone())
    }

    /// The package's `Depends:`, parsed lazily from `DESCRIPTION`. `None` when the file
    /// is missing. Narrow query over [`Package::description`], same backdating story as
    /// [`Package::version`].
    #[salsa::tracked(returns(ref))]
    pub fn depends(self, db: &dyn SourceDb) -> Option<Vec<String>> {
        self.description(db)
            .as_ref()
            .map(|description| description.depends.clone())
    }

    /// The basename order from `DESCRIPTION`'s `Collate:` field, parsed
    /// lazily. `None` when the field (or the file) is absent. Narrow query
    /// over [`Package::description`], same backdating story as
    /// [`Package::version`].
    #[salsa::tracked(returns(ref))]
    pub fn collation(self, db: &dyn SourceDb) -> Option<Vec<String>> {
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
    pub(crate) fn description(self, db: &dyn SourceDb) -> Option<Description> {
        // Depend on `description_revision()` so a bump forces a re-read
        report_untracked_if_zero(db, self.description_revision(db));

        let path = self.description_path(db).as_path()?;
        match db.read_to_string(path) {
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

    /// The package's parsed `INDEX`
    ///
    /// Returns `None` for workspace packages, which never have an `INDEX`.
    ///
    /// Returns an empty `Index` for installed packages with missing or unreadable
    /// indexes.
    #[salsa::tracked(returns(ref))]
    pub fn index(self, db: &dyn SourceDb) -> Option<Index> {
        // Depend on `index_revision()` so a bump forces a re-read.
        // Workspace packages don't have an `INDEX`.
        let revision = self.index_revision(db)?;
        report_untracked_if_zero(db, revision);

        let Some(parent) = self
            .description_path(db)
            .as_path()
            .and_then(|path| path.parent())
        else {
            log::error!("Failed to find `DESCRIPTION` parent for installed package.");
            return Some(Index::default());
        };

        let path = parent.join("INDEX");

        // The `index_revision()` early exit handled workspace packages, so we only handle
        // installed packages here. If an `INDEX` is missing, we silently return an empty
        // one ({translations} is an example). Otherwise, failure to parse logs an error.
        match db.read_to_string(&path) {
            Ok(text) => Some(Index::parse(&text)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Some(Index::default()),
            Err(err) => {
                log::error!("Failed to read `{path}`: {err:?}");
                Some(Index::default())
            },
        }
    }
}

/// Source files followed by generated files, with a private boundary between
/// them. Public construction accepts only source files, so callers cannot
/// supply an invalid boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageFiles {
    files: Vec<File>,

    /// `files[..sources_len]` are the source files, the rest generated.
    sources_len: usize,
}

impl PackageFiles {
    pub fn new(sources: Vec<File>) -> Self {
        Self {
            sources_len: sources.len(),
            files: sources,
        }
    }

    fn files(&self) -> &[File] {
        &self.files
    }

    fn sources(&self) -> &[File] {
        &self.files[..self.sources_len]
    }

    fn generated(&self) -> &[File] {
        &self.files[self.sources_len..]
    }

    /// Returns `None` for an unchanged source list, avoiding a Salsa input write.
    fn with_sources(&self, sources: Vec<File>) -> Option<Self> {
        if self.sources() == sources.as_slice() {
            return None;
        }
        let mut updated = Self::new(sources);
        updated.files.extend_from_slice(self.generated());
        Some(updated)
    }

    fn with_generated(&self, generated: Vec<File>) -> Option<Self> {
        if self.generated() == generated.as_slice() {
            return None;
        }
        let mut updated = Self::new(self.sources().to_vec());
        updated.files.extend(generated);
        Some(updated)
    }

    fn with_source_added(&self, file: File) -> Option<Self> {
        if self.files.contains(&file) {
            return None;
        }
        let mut updated = self.clone();
        updated.files.insert(self.sources_len, file);
        updated.sources_len += 1;
        Some(updated)
    }

    fn without(&self, file: File) -> Option<Self> {
        let index = self.files.iter().position(|candidate| *candidate == file)?;
        let mut updated = self.clone();
        updated.files.remove(index);
        if index < self.sources_len {
            updated.sources_len -= 1;
        }
        Some(updated)
    }
}

/// Compiled `exportPattern()` regexes of a package. See
/// [`Package::export_patterns()`].
#[derive(Debug, Clone)]
pub struct ExportPatterns {
    set: RegexSet,
}

impl ExportPatterns {
    fn new(patterns: &[String]) -> Self {
        let valid = patterns.iter().filter(|pattern| match Regex::new(pattern) {
            Ok(_) => true,
            Err(err) => {
                log::warn!("Skipping `exportPattern(\"{pattern}\")`: {err}");
                false
            },
        });
        let set = match RegexSet::new(valid) {
            Ok(set) => set,
            Err(err) => {
                log::error!("Failed to compile `exportPattern()` regexes: {err:?}");
                RegexSet::empty()
            },
        };
        Self { set }
    }

    /// Whether any pattern matches `name`. Like R, a pattern matches anywhere
    /// in the name unless it is anchored.
    pub fn is_match(&self, name: &str) -> bool {
        self.set.is_match(name)
    }
}

// Compare pattern text because it determines the compiled set.
impl PartialEq for ExportPatterns {
    fn eq(&self, other: &Self) -> bool {
        self.set.patterns() == other.set.patterns()
    }
}

impl Eq for ExportPatterns {}
