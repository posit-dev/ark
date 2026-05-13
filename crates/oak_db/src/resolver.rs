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
/// Each `source("path")` call triggers two reads on the target file, both
/// through narrow tracked queries:
///
/// - `target.exports(db)` for the names `source()` injects into the
///   calling scope.
///
/// - `target.attached_packages(db)` for the target's file-scope
///   `library()` calls. `source()` runs the target's top-level
///   statements at load time, so those attaches stick in the caller's
///   search path.
///
/// Both return PartialEq-stable values (a `FileExports` map and a
/// `Vec<String>` respectively), so body-only edits to the target backdate
/// at the narrow query layer and don't invalidate the caller.
///
/// Cycles in `source()` chains run through this resolver. Indexing A reads B's
/// queries, which read B.semantic_index, which (via this resolver) reads A's
/// queries, which read A.semantic_index. Both `File::semantic_index` and
/// `File::exports` have `cycle_result` handlers (FallbackImmediate). Salsa
/// breaks at whichever query it first re-enters; every cycle participant gets
/// the fallback. Each cycling file's exports surface ends up empty and its
/// attaches drop.
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
        let script = self.db.source_graph().script_by_url(self.db, &target_url)?;
        let target = script.file(self.db);

        let names: Vec<String> = target
            .exports(self.db)
            .iter()
            .map(|(name, _)| name.to_string())
            .collect();
        let packages: Vec<String> = target
            .attached_packages(self.db)
            .into_iter()
            .map(|name| name.text(self.db).to_string())
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
