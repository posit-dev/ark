use std::collections::HashMap;

use biome_rowan::TextSize;
use oak_semantic::semantic_index::SemanticCall;
use oak_semantic::semantic_index::SemanticCallKind;
use oak_semantic::ScopeId;

use crate::collation::collation_files;
use crate::Db;
use crate::File;
use crate::Name;
use crate::Package;
use crate::SourceNode;

/// A layer in a file's import chain.
///
/// Carries salsa entity ids (`File`, `Package`) end-to-end. No URL or
/// package-name strings cross out of `oak_db` for resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportLayer {
    /// A predecessor file in a package's collation, or another workspace
    /// file. Names are resolved through `file.exports(db)`.
    File(File),
    /// NAMESPACE `importFrom(pkg, name)` entries. Maps each imported
    /// symbol to its source package by name. Translation to `Package`
    /// happens at resolution time.
    PackageImports(HashMap<String, String>),
    /// An attached package's exports. Missing packages are filtered
    /// out by `imports`.
    PackageExports(Package),
}

/// The default R search path: packages always attached at startup, in
/// search order (last attached = searched first). Materialised as
/// `PackageExports` layers; packages absent from the source graph
/// drop out (the LSP fills them in later via `oak_sources`).
const DEFAULT_SEARCH_PATH: [&str; 7] = [
    "utils",
    "stats",
    "datasets",
    "methods",
    "grDevices",
    "graphics",
    "base",
];

#[salsa::tracked]
impl File {
    /// The import layers visible to this file at end-of-file, in R's
    /// lookup (LIFO) priority order. Symbols that don't have local
    /// bindings (are unbound in the file's semantic index) can be
    /// resolved against these imports.
    ///
    /// **Order is "most recent first".** The last `library()` call
    /// comes before the first and the latest collation file comes
    /// before the earliest. This makes the first hit in a forward
    /// search match R's runtime semantics (last attached / latest
    /// sourced wins).
    ///
    /// Offset-independent and stable across cursor moves. Recomputed
    /// only when the source graph, NAMESPACE, or this file's semantic
    /// calls actually change. See [`File::imports_at`] for the
    /// offset-narrowed subset of imports.
    #[salsa::tracked(returns(ref))]
    pub fn imports(self, db: &dyn Db) -> Vec<ImportLayer> {
        match self.parent(db) {
            Some(SourceNode::Package(package)) => package_layers(self, db, package),
            _ => script_layers(self, db),
        }
    }

    /// Import layers visible at an `offset` in a file:
    ///
    /// - **Cursor in lazy context**: returns the full lazy view. Lazy
    ///   contexts like functions are treated as if they run after the
    ///   file is fully sourced (over-approximation). Any `library()` /
    ///   collation entry is potentially visible regardless of where it
    ///   appears relative to the cursor.
    ///
    /// - **Top-level cursor (script)**: only `library()` calls that
    ///   have run by `offset` (file-scope calls before the cursor).
    ///   LIFO order, latest-attached comes first.
    ///
    /// - **Top-level cursor (package)**: only collation predecessors
    ///   of this file. LIFO order, latest-sourced predecessor comes
    ///   first. The package imports and base namespace come last.
    ///
    /// Plain method rather than `#[salsa::tracked]`. Tracking would
    /// key the cache on `(self, offset)`, creating one entry per
    /// cursor position. Skipping the cache is fine because the body
    /// just reads already-cached subqueries (`imports`,
    /// `semantic_index`) and applies an O(n) filter.
    pub fn imports_at(self, db: &dyn Db, offset: TextSize) -> Vec<ImportLayer> {
        let index = self.semantic_index(db);
        let file_scope = ScopeId::from(0);
        let (cursor_scope, _) = index.scope_at(offset);

        // Cursor in lazy context. EOF view, same as `imports()`.
        if cursor_scope != file_scope {
            return self.imports(db).clone();
        }

        // Top-level cursor: sequential narrowing.
        match self.parent(db) {
            Some(SourceNode::Package(package)) => narrow_package_top_level(self, db, package),
            _ => narrow_script_top_level(self, db, offset),
        }
    }
}

fn narrow_script_top_level(file: File, db: &dyn Db, offset: TextSize) -> Vec<ImportLayer> {
    let index = file.semantic_index(db);
    let file_scope = ScopeId::from(0);

    // Keep file-scope `library()` calls that have run by `offset`, in
    // LIFO order (latest-attached first).
    let mut layers: Vec<_> = index
        .semantic_calls()
        .iter()
        .rev()
        .filter(|call| call.scope() == file_scope && call.offset() < offset)
        .filter_map(|call| attach_layer(db, call))
        .collect();

    extend_with_default_search_path(db, &mut layers);
    layers
}

fn narrow_package_top_level(file: File, db: &dyn Db, package: Package) -> Vec<ImportLayer> {
    let collation = collation_files(db, package);
    let Some(file_pos) = collation.iter().position(|f| *f == file) else {
        // File claims membership but isn't in the collation. Populator
        // inconsistency. The package's `collation` and the file's
        // `parent` back-pointer drifted apart. Conservative fallback
        // with full imports.
        log::warn!(
            "File {} has package back-pointer to {} but is not in its collation",
            file.url(db),
            package.name(db),
        );
        return file.imports(db).clone();
    };

    let imports = file.imports(db);
    let Some(start) = imports
        .iter()
        .position(|l| matches!(l, ImportLayer::File(_)))
    else {
        // No File layers in `imports()` for a file that claims package
        // membership. Either the collation has only this file (and
        // self is excluded from imports, so the File block is empty)
        // or the populator is inconsistent. Conservative fallback:
        // full imports.
        return imports.clone();
    };

    // `imports()` emits the `collation.len() - 1` non-self collation
    // entries in a contiguous File block, in *reverse* collation
    // order (latest-sourced first). Successors of this file occupy
    // the first `(collation.len() - 1 - file_pos)` slots of that
    // block; predecessors occupy the remainder, in LIFO order. For a
    // top-level cursor we drop the successor prefix and keep
    // predecessors + the surrounding namespace / base layers.
    let drop_count = collation.len() - 1 - file_pos;
    imports[..start]
        .iter()
        .chain(&imports[start + drop_count..])
        .cloned()
        .collect()
}

fn attach_layer(db: &dyn Db, call: &SemanticCall) -> Option<ImportLayer> {
    let SemanticCallKind::Attach { package: name } = call.kind() else {
        return None;
    };
    package_by_name(db, name).map(ImportLayer::PackageExports)
}

fn script_layers(file: File, db: &dyn Db) -> Vec<ImportLayer> {
    let index = file.semantic_index(db);

    // Reverse: R searches LIFO, so latest-attached comes first.
    let mut layers: Vec<_> = index
        .semantic_calls()
        .iter()
        .rev()
        .filter_map(|call| attach_layer(db, call))
        .collect();
    extend_with_default_search_path(db, &mut layers);

    layers
}

fn package_layers(file: File, db: &dyn Db, package: Package) -> Vec<ImportLayer> {
    let mut layers = Vec::new();
    let namespace = package.namespace(db);

    // NAMESPACE `importFrom(pkg, name)` directives. Collect them into
    // a single layer that maps each imported symbol name to its source
    // package.
    if !namespace.imports.is_empty() {
        let map: HashMap<String, String> = namespace
            .imports
            .iter()
            .map(|imp| (imp.name.clone(), imp.package.clone()))
            .collect();
        layers.push(ImportLayer::PackageImports(map));
    }

    // NAMESPACE `import(pkg)` directives (bulk package imports).
    for pkg_name in &namespace.package_imports {
        if let Some(pkg) = package_by_name(db, pkg_name) {
            layers.push(ImportLayer::PackageExports(pkg));
        }
    }

    // Other collation files in *reverse* declaration order (LIFO,
    // latest-sourced first). Self is excluded: a file's own top-level
    // bindings come from `exports`, and including self here would
    // create a cycle in `resolve` for unbound names.
    for collation_file in collation_files(db, package).iter().rev() {
        if *collation_file == file {
            continue;
        }
        layers.push(ImportLayer::File(*collation_file));
    }

    // `base` is always implicitly available inside a package.
    if let Some(base) = package_by_name(db, "base") {
        layers.push(ImportLayer::PackageExports(base));
    }

    layers
}

fn extend_with_default_search_path(db: &dyn Db, layers: &mut Vec<ImportLayer>) {
    for pkg_name in DEFAULT_SEARCH_PATH {
        if let Some(pkg) = package_by_name(db, pkg_name) {
            layers.push(ImportLayer::PackageExports(pkg));
        }
    }
}

fn package_by_name(db: &dyn Db, name: &str) -> Option<Package> {
    db.source_graph()
        .package_by_name(db, Name::new(db, name))
}
