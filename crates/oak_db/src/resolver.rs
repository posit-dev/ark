use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use aether_url::UrlId;
use oak_semantic::ImportsResolver;
use oak_semantic::SourceResolution;
use url::Url;

use crate::Db;
use crate::File;

/// Salsa-backed [`ImportsResolver`] consumed by the per-file semantic
/// index builder. One instance per call to [`File::semantic_index`].
///
/// Lookups go through the source graph (`script_by_url`) and through the
/// target file's own [`File::semantic_index`] (for exported names and
/// attached packages). Each `source("path")` call records a salsa dep
/// on the target file's semantic index, not on its parse tree or
/// contents. That dep is coarse today. `SemanticIndex` is not
/// `PartialEq`-stable across internal edits (AstPtr ranges shift), so
/// any edit to a sourced file invalidates this file's index too.
///
/// TODO(salsa): once `File::exports` lands and `DbResolver` reads
/// `target.exports(db)` here instead of
/// `target.semantic_index().file_exports()`, the dep moves to a
/// structurally stable shape (name to `ExportEntry`, no source
/// ranges). Body-only edits to sourced files stop invalidating
/// callers, and salsa backdates as the design note advertises.
pub(crate) struct DbResolver<'db> {
    db: &'db dyn Db,
    /// The file currently being indexed. The resolver's whole job is to
    /// answer import queries on its behalf. Today the only such query
    /// is `resolve_source()`.
    calling_file: File,
}

impl<'db> DbResolver<'db> {
    pub(crate) fn new(db: &'db dyn Db, calling_file: File) -> Self {
        Self { db, calling_file }
    }
}

impl<'db> ImportsResolver for DbResolver<'db> {
    fn resolve_source(&mut self, path: &str) -> Option<SourceResolution> {
        let anchor = anchor_dir(self.db, self.calling_file)?;
        let target_url = resolve_relative_to(&anchor, path)?;
        let script = crate::script_by_url(self.db, &target_url)?;
        let target = script.file(self.db);

        // Reads the target's own `semantic_index`. Salsa records the dep
        // edge; cycles in `source()` chains are caught by the cycle_result
        // on `File::semantic_index`.
        //
        // TODO(salsa): switch to `target.exports(self.db)` once that
        // tracked query lands. Same change moves the cycle handler off
        // `semantic_index` (finer recovery) and makes the dep edge
        // here PartialEq-stable across body-only edits.
        let index = target.semantic_index(self.db);

        let names: Vec<String> = index
            .file_exports()
            .keys()
            .map(|name| name.to_string())
            .collect();
        let packages: Vec<String> = index
            .file_attached_packages()
            .into_iter()
            .map(|s| s.to_string())
            .collect();

        Some(SourceResolution {
            file: target_url.as_url().clone(),
            names,
            packages,
        })
    }
}

/// Anchor directory for relative `source("path")` arguments.
///
/// Workspace root if the file is under one, else the file's parent directory. R
/// resolves `source("foo.R")` against `getwd()`, and IDEs (RStudio, Positron)
/// `setwd()` to the project root, so workspace-root anchoring typically matches
/// the runtime behaviour.
fn anchor_dir(db: &dyn Db, calling_file: File) -> Option<PathBuf> {
    if let Some(root) = calling_file.workspace_root(db) {
        return root.path(db).to_file_path();
    }
    let calling_path = calling_file.url(db).to_file_path()?;
    calling_path.parent().map(PathBuf::from)
}

/// Resolve `path` (the literal `source("path")` argument) against the anchor
/// directory. Applies pure `..` / `.` normalisation (no I/O). Returns `None` if
/// the joined path can't be turned back into a file URL.
fn resolve_relative_to(anchor_dir: &Path, path: &str) -> Option<UrlId> {
    // `from_file_path` failures are expected for ill-formed paths.
    // Drop silently rather than logging noise during discovery.
    let raw: PathBuf = anchor_dir.join(path);
    let target_path = normalise_path(&raw);
    let url = Url::from_file_path(&target_path).ok()?;
    Some(UrlId::from_canonical(url))
}

/// Resolve `..` and `.` components in `path` lexically, without
/// touching the filesystem. Mirrors `Path::canonicalize` minus the
/// symlink walk. Leading `..` against the root just drops (the root
/// has no parent).
fn normalise_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {},
            Component::ParentDir => {
                if !out.pop() {
                    // Already at the root (or before the prefix /
                    // root component); leading `..` has nothing to
                    // pop, so drop it.
                }
            },
            other => out.push(other.as_os_str()),
        }
    }
    out
}
