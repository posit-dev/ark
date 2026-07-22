use aether_path::FilePath;
use camino::Utf8Component;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use oak_semantic::effects_registry;
use oak_semantic::EffectsHandlers;
use oak_semantic::ImportsResolver;
use oak_semantic::SourceResolution;
use url::Url;

use crate::Db;
use crate::File;
use crate::RootKind;

/// Salsa-backed [`ImportsResolver`] consumed by the per-file semantic
/// index builder. One instance per call to [`File::semantic_index`].
///
/// Each `source("path")` call triggers two reads on the target file, both
/// through narrow tracked queries:
///
/// - `target.exports(db)` for the names `source()` injects into the
///   calling scope.
///
/// - `target.attached_packages(db)` for the target's top-level `library()`
///   calls, the ones `source()` actually runs. A `library()` buried in a
///   function body has not run at source time, so it is excluded.
///
/// Both return PartialEq-stable values (a `FileExports` map and a
/// `Vec<String>` respectively), so body-only edits to the target backdate
/// at the narrow query layer and don't invalidate the caller.
///
/// Cycles in `source()` chains run through this resolver:
/// `semantic_index(A)` reads `exports(B)`, which reads `semantic_index(B)`,
/// which reads `exports(A)`, which reads `semantic_index(A)`. Each of
/// `semantic_index()`, `exports()`, `imports()`, and `resolve()` carries its
/// own `cycle_result`. See [`File::semantic_index`]'s doc for the asymmetric
/// recovery behaviour (custom rebuild on `semantic_index()`, empty fallback
/// on the narrow queries).
pub(crate) struct SalsaImportsResolver<'db> {
    db: &'db dyn Db,
    /// The file currently being indexed. The resolver's whole job is to
    /// answer import queries on its behalf. Today the only such query
    /// is `resolve_source()`.
    calling_file: File,
}

impl<'db> SalsaImportsResolver<'db> {
    pub(crate) fn new(db: &'db dyn Db, calling_file: File) -> Self {
        Self { db, calling_file }
    }
}

impl<'db> ImportsResolver for SalsaImportsResolver<'db> {
    fn resolve_source(&mut self, path: &str) -> Option<SourceResolution> {
        let anchor = anchor_dir(self.db, self.calling_file)?;
        let target_path = resolve_relative_to(&anchor, path)?;
        // TODO: a `source()` target outside every workspace root never becomes
        // a `File`, so `file_by_path()` misses it and the names it injects stay
        // invisible. Minting can't happen here, so the work belongs on the
        // write side in `oak_scan`. We should carry the resolved path on the
        // directive even when no `File` exists (today the miss returns `None`
        // and drops it), then have `oak_scan` enumerate source directives after
        // a scan, mint an `OrphanRoot` `File` from disk for each
        // out-of-workspace target, and iterate for `source()` chains. A file
        // watcher is only needed for freshness (re-reading after an external
        // edit), plus GC to drop the orphan once the directive goes away.
        // TODO(diagnostics): Until we support out-of-workspace sourced files,
        // should we at least lint so user knows that we can't analyse the file?
        let file = self.db.file_by_path(&target_path)?;

        let names: Vec<String> = file
            .exports(self.db)
            .iter()
            .map(|(name, _)| name.to_string())
            .collect();

        let packages: Vec<String> = file
            .attached_packages(self.db)
            .iter()
            .map(|name| name.text(self.db).to_string())
            .collect();

        Some(SourceResolution {
            url: target_path.to_url(),
            names,
            packages,
        })
    }

    fn resolve_effects(
        &mut self,
        name: &str,
        _attached: &[String],
        _lazy: bool,
    ) -> Option<EffectsHandlers> {
        // Base is the always-attached layer at the bottom of the search path,
        // resolved through the same registry lookup as any package.
        //
        // TODO!: walk the rest of the search path too (flow-order attaches,
        // package siblings via `who_defines`, NAMESPACE imports, re-export chase).
        effects_registry::lookup("base", name).copied()
    }
}

/// Anchor directory for relative `source("path")` arguments.
///
/// Workspace root if the file is under one, else the file's parent directory. R
/// resolves `source("foo.R")` against `getwd()`, and IDEs (RStudio, Positron)
/// `setwd()` to the project root, so workspace-root anchoring typically matches
/// the runtime behaviour.
fn anchor_dir(db: &dyn Db, calling_file: File) -> Option<Utf8PathBuf> {
    if let Some(root) = calling_file
        .root(db)
        .filter(|r| r.kind(db) == RootKind::Workspace)
    {
        // Workspace roots are file URLs by construction.
        return root.path(db).as_path().map(Utf8Path::to_path_buf);
    }

    let parent = calling_file.path(db).as_path()?.parent()?;
    Some(parent.to_path_buf())
}

/// Resolve `path` (the literal `source("path")` argument) against the anchor
/// directory. Applies pure `..` / `.` normalisation (no I/O). Returns `None` if
/// the joined path can't be turned back into a file URL.
fn resolve_relative_to(anchor_dir: &Utf8Path, path: &str) -> Option<FilePath> {
    // `Url::from_file_path` failures are expected for ill-formed paths.
    // Drop silently rather than logging noise during discovery.
    let raw = anchor_dir.join(path);
    let target_path = normalise_path(&raw);
    let url = Url::from_file_path(target_path.as_std_path()).ok()?;
    Some(FilePath::from_url(&url))
}

/// Resolve `..` and `.` components in `path` lexically, without
/// touching the filesystem. Mirrors `Path::canonicalize` minus the
/// symlink walk. Leading `..` against the root just drops (the root
/// has no parent).
fn normalise_path(path: &Utf8Path) -> Utf8PathBuf {
    let mut out = Utf8PathBuf::new();
    for component in path.components() {
        match component {
            Utf8Component::CurDir => {},
            Utf8Component::ParentDir => {
                if !out.pop() {
                    // Already at the root (or before the prefix /
                    // root component); leading `..` has nothing to
                    // pop, so drop it.
                }
            },
            other => out.push(other.as_str()),
        }
    }
    out
}
