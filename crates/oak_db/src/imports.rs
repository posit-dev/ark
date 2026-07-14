use std::ops::ControlFlow;

use aether_path::FilePath;
use camino::Utf8Component;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use oak_semantic::effects_registry;
use oak_semantic::EffectsHandlers;
use oak_semantic::ImportsResolver;
use oak_semantic::SourceResolution;
use url::Url;

use crate::file_imports::CollationView;
use crate::file_imports::ImportLayer;
use crate::Db;
use crate::File;
use crate::Package;
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
    /// The file currently being indexed.
    file: File,
}

impl<'db> SalsaImportsResolver<'db> {
    pub(crate) fn new(db: &'db dyn Db, file: File) -> Self {
        Self { db, file }
    }
}

impl<'db> ImportsResolver for SalsaImportsResolver<'db> {
    fn resolve_source(&mut self, path: &str) -> Option<SourceResolution> {
        let anchor = anchor_dir(self.db, self.file)?;
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
        attached: &[String],
        _lazy: bool,
    ) -> Option<EffectsHandlers> {
        // Walk the same load-time layer chain as `File::resolve`, but map each
        // layer to an NSE effect instead of a definition.
        //
        // Always the eager (predecessors-only) view, even for a lazy callee. A
        // top-level callee only sees names loaded before it, so eager is exact
        // there. A lazy callee (a function body) runs after the whole package
        // has loaded, so R would resolve it against every sibling, and
        // `File::resolve` does use the lazy view for that case. We can't here.
        // This runs while the file's own index is being built, and the lazy
        // view would read a collation successor's `exports`, whose own build
        // reads back into this file and cycles (salsa recovers with empty
        // exports, so the extra shadow detection it would buy is degraded
        // anyway). A later sibling that shadows a lazy NSE call is missed here,
        // and is linted later on.
        let layers = self.file.cross_file_layers(self.db, CollationView::Eager);

        // The file's own attaches slot between the definition/namespace band
        // and the rest of the search path, exactly as in `File::imports`.
        // `attached` is the builder's flow-ordered set (latest last), so
        // eager/lazy flow-sensitivity is already applied; reverse it to LIFO so
        // a later attach shadows an earlier one.
        let own: Vec<ImportLayer> = attached
            .iter()
            .rev()
            .filter_map(|package| self.db.package_by_name(package).map(ImportLayer::Package))
            .collect();

        for layer in layers.splice_own_attaches(own) {
            if let ControlFlow::Break(effect) = self.layer_effect(&layer, name) {
                return effect;
            }
        }

        // base is the bottom of every R search path and is present in any
        // session, so its builtins (`library`, `source`, `quote`, `local`, ...)
        // resolve by name here even when base isn't scanned into a root. A
        // definition or a higher package on the path shadows it, which the walk
        // above already handled before falling through.
        effects_registry::lookup("base", name).copied()
    }
}

/// What a package layer contributes for `name` as the walk reaches it.
enum PackageBinding {
    /// Binds `name` and it carries an effect.
    Effect(EffectsHandlers),
    /// Binds `name` (exports it) but with no known effect, e.g. a plain
    /// exported function. It still shadows any same-named effect deeper on the
    /// search path, so the walk stops here with no effect.
    Shadow,
    /// Doesn't bind `name`, the walk keeps going.
    Absent,
}

impl<'db> SalsaImportsResolver<'db> {
    /// Project one import layer to an NSE effect, the effects-side twin of
    /// `resolve_import_layer` in `file_resolve`. Both reduce the layer to the
    /// package it binds `name` to and split on the same cases. Only the
    /// projection differs (definition there, effect here).
    ///
    /// Shadowing is export-driven, matching how the definition side stops at the
    /// first package that resolves `name`. `Break(Some(effect))` found an
    /// effect. `Break(None)` hit a binding with no effect (a sibling definition,
    /// a plain package export, a namespace import), which shadows any deeper
    /// effect, so the bare call is not NSE. `Continue` means the layer doesn't
    /// bind `name`; keep walking.
    fn layer_effect(
        &self,
        layer: &ImportLayer,
        name: &str,
    ) -> ControlFlow<Option<EffectsHandlers>> {
        match layer {
            // A definition shadows any deeper effect. Own-file definitions never
            // reach here, the builder handles them before calling us.
            ImportLayer::File(file) => match file.exports(self.db).get(name).is_some() {
                true => ControlFlow::Break(None),
                false => ControlFlow::Continue(()),
            },
            ImportLayer::Package(package) => match self.package_binding(*package, name) {
                PackageBinding::Effect(effects) => ControlFlow::Break(Some(effects)),
                PackageBinding::Shadow => ControlFlow::Break(None),
                PackageBinding::Absent => ControlFlow::Continue(()),
            },
            // A NAMESPACE `importFrom` binds `name` unconditionally (that's what
            // the directive asserts), so it always shadows the search path
            // below. Its effect, if any, comes from the source package.
            ImportLayer::From(map) => match map.get(name) {
                Some(source) => {
                    let effect = self.db.package_by_name(source).and_then(|package| {
                        match self.package_binding(package, name) {
                            PackageBinding::Effect(effects) => Some(effects),
                            PackageBinding::Shadow | PackageBinding::Absent => None,
                        }
                    });
                    ControlFlow::Break(effect)
                },
                None => ControlFlow::Continue(()),
            },
        }
    }

    /// How `package` binds `name`: a direct registry effect, a plain export that
    /// only shadows, or nothing. The re-export chase is one hop through an
    /// `importFrom`, since a re-exported function's annotation lives under its
    /// original package, not the re-exporter.
    fn package_binding(&self, package: Package, name: &str) -> PackageBinding {
        let package_name = package.name(self.db).as_str();
        if let Some(effects) = effects_registry::lookup(package_name, name) {
            return PackageBinding::Effect(*effects);
        }
        // base is the terminal layer, so it has nothing below to shadow, and we
        // don't carry its full builtin export list. Treat it as unbound here;
        // its effects resolve through the registry lookup above (and the base
        // fallthrough in `resolve_effects`).
        if package_name == "base" {
            return PackageBinding::Absent;
        }
        // The package binds `name` only when it exports it. This is the same
        // export gate `Package::resolve` applies. A name it `importFrom`s
        // without re-exporting isn't visible to a caller that attaches or
        // imports this package (R errors "could not find function").
        let namespace = package.namespace(self.db);
        if !namespace.exports.contains_str(name) {
            return PackageBinding::Absent;
        }
        // Exports `name`, so it binds. Chase a re-export for the effect; a plain
        // own definition (no matching `importFrom`) only shadows.
        match namespace.imports.iter().find(|import| import.name == name) {
            Some(import) => match effects_registry::lookup(&import.package, name) {
                Some(effects) => PackageBinding::Effect(*effects),
                None => PackageBinding::Shadow,
            },
            None => PackageBinding::Shadow,
        }
    }
}

/// Anchor directory for relative `source("path")` arguments.
///
/// Workspace root if the file is under one, else the file's parent directory. R
/// resolves `source("foo.R")` against `getwd()`, and IDEs (RStudio, Positron)
/// `setwd()` to the project root, so workspace-root anchoring typically matches
/// the runtime behaviour.
fn anchor_dir(db: &dyn Db, file: File) -> Option<Utf8PathBuf> {
    if let Some(root) = file.root(db).filter(|r| r.kind(db) == RootKind::Workspace) {
        // Workspace roots are file URLs by construction.
        return root.path(db).as_path().map(Utf8Path::to_path_buf);
    }

    let parent = file.path(db).as_path()?.parent()?;
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
