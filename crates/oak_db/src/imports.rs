use aether_path::FilePath;
use camino::Utf8Component;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use oak_semantic::effects;
use oak_semantic::effects::DirWalk;
use oak_semantic::FunctionHandlers;
use oak_semantic::ImportsResolver;
use oak_semantic::SourceResolution;
use rustc_hash::FxHashMap;
use url::Url;

use crate::directory::files_in_directory;
use crate::directory::files_in_directory_recursive;
use crate::file_imports::CollationView;
use crate::file_imports::ImportLayer;
use crate::Db;
use crate::File;
use crate::Name;
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
/// which reads `exports(A)`, which reads `semantic_index(A)`. `semantic_index()`
/// and `exports()` carry the `cycle_result` handlers that break it. See
/// [`File::semantic_index`]'s doc for the recovery behaviour (custom rebuild on
/// `semantic_index()`, empty fallback on `exports()`).
pub(crate) struct SalsaImportsResolver<'db> {
    db: &'db dyn Db,
    /// The file currently being indexed.
    file: File,
    cache: EffectsCache,
}

impl<'db> SalsaImportsResolver<'db> {
    pub(crate) fn new(db: &'db dyn Db, file: File) -> Self {
        Self {
            db,
            file,
            cache: EffectsCache::default(),
        }
    }

    /// What sourcing `file` brings in. The two reads this makes are the ones
    /// described on [`SalsaImportsResolver`].
    fn source_resolution(&self, file: File) -> SourceResolution {
        // Sort to prevent an unrelated export from renumbering `Import`
        // definitions. `record_binding()` anchors every name at the same
        // `source()` call, so the original order has no semantic meaning.
        let mut names: Vec<String> = file
            .exports(self.db)
            .iter()
            .map(|(name, _)| name.to_string())
            .collect();
        names.sort();

        let packages: Vec<String> = file
            .attached_packages(self.db)
            .iter()
            .map(|name| name.text(self.db).to_string())
            .collect();

        SourceResolution {
            url: file.path(self.db).to_url(),
            names,
            packages,
        }
    }
}

/// Returns scripts from the workspace directory named by `path`, in load order.
/// `walk` determines whether nested directories are included. Returns no files
/// when `path` cannot resolve to a directory.
///
/// Tracked to give the directory listing a backdating point, the role
/// [`File::collation_siblings`] plays for the `R/` convention. The listing
/// filters every root's scripts, so without a memo here a file appearing
/// anywhere in the workspace would re-run the whole `semantic_index` of every
/// file holding a directory source, `_targets.R` being the one that hurts.
///
/// Reads only inputs, so it's safe to call while `file`'s own index is being
/// built.
#[salsa::tracked(returns(ref))]
pub(crate) fn source_dir_scripts(
    db: &dyn Db,
    file: File,
    path: String,
    walk: DirWalk,
) -> Vec<File> {
    let Some(anchor) = anchor_dir(db, file) else {
        return Vec::new();
    };
    let Some(target_path) = resolve_relative_to(&anchor, &path) else {
        return Vec::new();
    };
    let Some(dir) = target_path.as_path() else {
        return Vec::new();
    };
    match walk {
        DirWalk::Shallow => files_in_directory(db, dir),
        DirWalk::Recursive => files_in_directory_recursive(db, dir),
    }
}

/// Per-build memo for `resolve_effects`, keyed on `(name, attached)`.
/// Sound because the answer only depends on the frozen db, and a
/// `SalsaImportsResolver` lives exactly as long as one `File::semantic_index`
/// build.
#[derive(Default)]
struct EffectsCache {
    entries: FxHashMap<Vec<String>, FxHashMap<String, Option<FunctionHandlers>>>,
}

impl EffectsCache {
    fn get(&self, name: &str, attached: &[String]) -> Option<Option<FunctionHandlers>> {
        self.entries.get(attached)?.get(name).copied()
    }

    fn insert(&mut self, name: &str, attached: &[String], effects: Option<FunctionHandlers>) {
        self.entries
            .entry(attached.to_vec())
            .or_default()
            .insert(name.to_string(), effects);
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
        Some(self.source_resolution(file))
    }

    fn resolve_source_dir(&mut self, path: &str, walk: DirWalk) -> Vec<SourceResolution> {
        source_dir_scripts(self.db, self.file, path.to_string(), walk)
            .iter()
            .copied()
            // Exclude sourcing file
            .filter(|file| *file != self.file)
            .map(|file| self.source_resolution(file))
            .collect()
    }

    fn resolve_effects(&mut self, name: &str, attached: &[String]) -> Option<FunctionHandlers> {
        if let Some(effects) = self.cache.get(name, attached) {
            return effects;
        }
        let effects = self.resolve_effects_uncached(name, attached);
        self.cache.insert(name, attached, effects);
        effects
    }

    /// Treat file bindings as assignable so `.onLoad()` can use `<<-` before
    /// the namespace is sealed. This misses runtime errors from function bodies
    /// called after sealing. Script bindings live in the global environment
    /// and are not locked by package loading.
    ///
    /// FIXME A sibling file binding can hide a locked package target from
    /// top-level `<<-`. R skips the environment executing the assignment,
    /// including sibling bindings that share it, but `File` layers do not
    /// identify environments. Shiny's `R/` files share one environment, whereas
    /// `global.R` runs in an enclosing environment and must remain searchable.
    fn binding_package(&mut self, name: &str, attached: &[String]) -> Option<String> {
        let layers = self.file.cross_file_layers(self.db, CollationView::Eager);
        let own = own_attach_layers(self.db, attached);

        let binding = layers
            .lookup_order(self.db, &own)
            .find_map(|layer| layer_binding(self.db, &layer, name));

        match binding {
            Some(LayerBinding::File) => None,
            // For base's own files, the base layer is their own namespace,
            // whose bindings are assignable like any namespace sibling's.
            Some(LayerBinding::Package { package, .. })
                if package == "base" && self.indexes_base() =>
            {
                None
            },
            Some(LayerBinding::Package { package, .. }) => Some(package),
            None => self.base_binding_package(name),
        }
    }

    fn package_exists(&mut self, package: &str) -> bool {
        self.db.package_by_name(package).is_some()
    }
}

impl<'db> SalsaImportsResolver<'db> {
    /// Walks the same load-time layer chain as `File::resolve`, but maps each
    /// layer to an NSE effect instead of a definition.
    ///
    /// Always the eager (predecessors-only) view, even for a lazy callee. A
    /// top-level callee only sees names loaded before it, so eager is exact
    /// there. A lazy callee (a function body) runs after the whole package
    /// has loaded, so R would resolve it against every sibling, and
    /// `File::resolve` does use the lazy view for that case. We can't here.
    /// This runs while the file's own index is being built, and the lazy
    /// view would read a collation successor's `exports`, whose own build
    /// reads back into this file and cycles (salsa recovers with empty
    /// exports, so the extra shadow detection it would buy is degraded
    /// anyway).
    ///
    /// A later sibling that shadows a lazy NSE call is missed here, and nothing
    /// flags it.
    ///
    /// TODO(diagnostics): Detect it in the post-index diagnostics query, where
    /// reading a successor's `exports` doesn't cycle. Unlike the ambiguities the
    /// builder records, this one can't be seen from inside the file.
    fn resolve_effects_uncached(
        &self,
        name: &str,
        attached: &[String],
    ) -> Option<FunctionHandlers> {
        let layers = self.file.cross_file_layers(self.db, CollationView::Eager);

        // The file's own attaches slot between the definition/namespace band
        // and the rest of the search path, exactly as in `File::imports`.
        // `attached` is the builder's flow-ordered set (latest last), so
        // eager/lazy flow-sensitivity is already applied; reverse it to LIFO so
        // a later attach shadows an earlier one.
        let own = own_attach_layers(self.db, attached);

        let binding = layers
            .lookup_order(self.db, &own)
            .find_map(|layer| layer_binding(self.db, &layer, name));
        binding_handlers(binding, name).copied()
    }

    /// Check base's source bindings for locks even when they have no registry
    /// handlers. `layer_binding()` recognizes only registry names in base,
    /// which has no `NAMESPACE`. Effect lookup needs no broader check because
    /// base is last in lookup order and cannot shadow a deeper effect.
    ///
    /// Skip this check for base's own files. `base_binds()` reads their exports,
    /// which depend on the semantic index being built and would cycle.
    fn base_binding_package(&self, name: &str) -> Option<String> {
        if self.indexes_base() {
            return None;
        }
        let base = self.db.package_by_name("base")?;
        base_binds(self.db, base, Name::new(self.db, name)).then(|| String::from("base"))
    }

    fn indexes_base(&self) -> bool {
        self.file
            .package(self.db)
            .is_some_and(|package| package.name(self.db) == "base")
    }
}

/// Later attachments shadow earlier ones. The builder supplies runtime order
/// (latest last), while layer lookup needs the most recent attachment first.
fn own_attach_layers(db: &dyn Db, attached: &[String]) -> Vec<ImportLayer> {
    attached
        .iter()
        .rev()
        .filter_map(|package| db.package_by_name(package).map(ImportLayer::Package))
        .collect()
}

/// Base has no `NAMESPACE`, so top-level file bindings stand in for exports.
/// This misses primitives such as `sum()`, which have no R-level definition.
///
/// Salsa caches the scan per package and name, invalidating it when the file
/// list or an export set read by the query changes.
#[salsa::tracked]
fn base_binds<'db>(db: &'db dyn Db, base: Package, name: Name<'db>) -> bool {
    let name = name.text(db).as_str();
    base.files(db)
        .iter()
        .any(|file| file.exports(db).get(name).is_some())
}

/// Resolves a bare `name` call to its NSE effect through `layers`.
///
/// Falls through to base's builtins when no layer binds `name`, matching
/// [`SalsaImportsResolver::resolve_effects_uncached`].
pub(crate) fn resolve_effect(
    db: &dyn Db,
    layers: &[ImportLayer],
    name: &str,
) -> Option<&'static FunctionHandlers> {
    let binding = layers
        .iter()
        .find_map(|layer| layer_binding(db, layer, name));
    binding_handlers(binding, name)
}

/// A binding without handlers shadows any deeper effect. Base's registered
/// builtins remain available when no layer binds `name`, even when base isn't
/// scanned into a root, because base is present at the bottom of R's search path.
fn binding_handlers(
    binding: Option<LayerBinding>,
    name: &str,
) -> Option<&'static FunctionHandlers> {
    match binding {
        Some(LayerBinding::File) => None,
        Some(LayerBinding::Package { handlers, .. }) => handlers,
        None => effects::lookup("base", name),
    }
}

enum LayerBinding {
    File,
    /// For `importFrom`, `package` identifies the source package rather than
    /// the importer. `handlers` preserves registry identity for comparison.
    Package {
        package: String,
        handlers: Option<&'static FunctionHandlers>,
    },
}

fn layer_binding(db: &dyn Db, layer: &ImportLayer, name: &str) -> Option<LayerBinding> {
    match layer {
        // The builder handles own-file definitions before calling the resolver,
        // so they do not participate in this layer lookup.
        ImportLayer::File(file) => file
            .exports(db)
            .get(name)
            .is_some()
            .then_some(LayerBinding::File),
        // These layers cannot occur while the resolver builds the file's index.
        // `build_inherited_layers()` creates them only after the index exists.
        ImportLayer::SourcingFile {
            file,
            exports_so_far,
        } => (exports_so_far.contains(name) && file.exports(db).get(name).is_some())
            .then_some(LayerBinding::File),
        ImportLayer::Package(package) => {
            let handlers = package_handlers(db, *package, name);
            if handlers.is_none() && !package_exports(db, *package, name) {
                return None;
            }
            Some(LayerBinding::Package {
                package: package.name(db).to_string(),
                handlers,
            })
        },
        // `importFrom` shadows deeper search-path bindings even when the source
        // package is unavailable or has no registered handlers for `name`.
        ImportLayer::From(importer) => {
            let source = importer.imported_from(db).get(name)?;
            let handlers = db
                .package_by_name(source)
                .and_then(|package| package_handlers(db, package, name));
            Some(LayerBinding::Package {
                package: source.to_string(),
                handlers,
            })
        },
    }
}

/// Follow re-exports one `importFrom` hop because a re-exported function's
/// handlers are registered under its source package, not the re-exporter.
///
/// A registry entry implies a binding, so a `Some` here binds `name` even
/// when export data is missing.
fn package_handlers(
    db: &dyn Db,
    package: Package,
    name: &str,
) -> Option<&'static FunctionHandlers> {
    if let Some(handlers) = effects::lookup(package.name(db), name) {
        return Some(handlers);
    }
    if !package_exports(db, package, name) {
        return None;
    }
    let source = package.imported_from(db).get(name)?;
    effects::lookup(source, name)
}

/// Imported names are invisible to callers unless re-exported, matching
/// the export gate in [`Package::resolve()`].
///
/// Base has no `NAMESPACE` or full builtin list here, so this check excludes it.
/// `layer_binding()` recognizes its registry entries separately. Other base
/// bindings cannot shadow deeper effects because base is last in lookup order,
/// but lock checks still need them and use `base_binding_package()`.
fn package_exports(db: &dyn Db, package: Package, name: &str) -> bool {
    if package.name(db) == "base" {
        return false;
    }
    package.namespace(db).exports.contains_str(name)
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
                // A `..` with nothing to pop is at the root (or before the
                // prefix / root component). Just drop.
                out.pop();
            },
            other => out.push(other.as_str()),
        }
    }
    out
}
