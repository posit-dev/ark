use std::ptr;

use oak_semantic::EffectsHandlers;
use rustc_hash::FxHashSet;

use crate::diagnostic::Diagnostic;
use crate::diagnostic::DiagnosticKind;
use crate::file_imports::CollationView;
use crate::file_imports::ImportLayer;
use crate::file_resolve::resolve_import_layer;
use crate::imports::resolve_effect;
use crate::Db;
use crate::File;
use crate::Name;

/// Diagnostics for effect ambiguities produced by source inheritance.
///
/// ```r
/// # main.R                     # helpers.R
/// source <- identity           source("more.R")
/// base::source("helpers.R")
/// ```
///
/// `helpers.R` is analysed on its own, so its `source("more.R")` reads as base
/// `source` and we follow it. But `main.R` binds `source` to `identity` before
/// sourcing `helpers.R`, so at runtime that call might do nothing at all.
/// We report this ambiguity with a diagnostic.
///
/// A call is ambiguous when [`File::imports`] resolves it to a different effect
/// than [`File::standalone_imports`].
pub(crate) fn inherited_shadow_diagnostics(db: &dyn Db, file: File) -> Vec<Diagnostic> {
    if file.inherited_layers(db, CollationView::Lazy).is_empty() {
        return Vec::new();
    }

    let inherited = file.imports(db);
    let standalone = file.standalone_imports(db);

    let mut reported = FxHashSet::default();
    let mut diagnostics = Vec::new();

    for call in file.semantic_index(db).semantic_calls() {
        let Some(callee) = call.callee() else {
            continue;
        };
        // One `source()` call forwards one `Attach` per package attached in the
        // target, all sharing its range. Report the site once.
        if !reported.insert(call.range()) {
            continue;
        }

        let name = Name::new(db, callee);

        // A binding in this file wins on both sides, so there's nothing to
        // disagree about. This is the same first step `File::resolve` takes.
        if !file.resolve_export(db, name).is_empty() {
            continue;
        }

        let callee_text = name.text(db);
        if same_effect(
            resolve_effect(db, inherited, callee_text.as_str()),
            resolve_effect(db, &standalone, callee_text.as_str()),
        ) {
            continue;
        }

        let Some(resolved_layer) = resolve_layer(db, inherited, name) else {
            continue;
        };

        let Some(sourcing) = sourcing_file(db, file, &resolved_layer) else {
            continue;
        };

        // When nothing on the standalone path binds the callee, the scan fell
        // through to base's builtins, which resolve by name whether or not base
        // was scanned into a root. See `SalsaImportsResolver`.
        let alone = match resolve_layer(db, &standalone, name) {
            Some(layer) => describe_source(db, &layer),
            None => "package `base`".to_string(),
        };

        diagnostics.push(Diagnostic::new(
            DiagnosticKind::InheritedShadow,
            format!(
                "This `{callee}` call has an ambiguous effect. It resolves through {alone} when \
                 the file is sourced on its own, and to {inherited} when sourced by `{sourcing}`.",
                inherited = describe_source(db, &resolved_layer),
            ),
            call.range(),
            Vec::new(),
        ));
    }

    diagnostics
}

/// Whether two layer chains resolve a bare call to the same effect.
///
/// The effect registry provides one static [`EffectsHandlers`] per
/// `(package, function)`, so pointer identity is sufficient.
///
/// TODO(declarations): Only attach and source callees reach this comparison,
/// and no two packages currently register the same callee. Because of this,
/// we're missing test coverage. We should complete test coverage once local
/// declaration of effects lands.
fn same_effect(
    left: Option<&'static EffectsHandlers>,
    right: Option<&'static EffectsHandlers>,
) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => ptr::eq(left, right),
        (None, None) => true,
        _ => false,
    }
}

/// The first layer that binds `name`, i.e. the one a lookup would settle on.
fn resolve_layer<'db>(
    db: &'db dyn Db,
    layers: &[ImportLayer],
    name: Name<'db>,
) -> Option<ImportLayer> {
    layers
        .iter()
        .find(|layer| !resolve_import_layer(db, layer, name).is_empty())
        .cloned()
}

/// The name of the file whose inherited band contributed `layer`.
///
/// Names the direct sourcing file even for a layer that reaches `file` from
/// further up the chain, since [`build_inherited_layers`] folds each site's
/// grandparents into that site's own bands.
///
/// [`build_inherited_layers`]: crate::file_imports
fn sourcing_file(db: &dyn Db, file: File, layer: &ImportLayer) -> Option<String> {
    file.inherited_layers(db, CollationView::Lazy)
        .iter()
        .find(|site| site.layers.enclosing.contains(layer) || site.layers.attaches.contains(layer))
        .map(|site| {
            site.file
                .path(db)
                .file_name()
                .unwrap_or_default()
                .to_string()
        })
}

fn describe_source(db: &dyn Db, layer: &ImportLayer) -> String {
    match layer {
        ImportLayer::File(file) | ImportLayer::SourcingFile { file, .. } => {
            format!(
                "a binding in `{}`",
                file.path(db).file_name().unwrap_or_default()
            )
        },
        ImportLayer::Package(package) => format!("package `{}`", package.name(db)),
        ImportLayer::From(importer) => format!("an import of `{}`", importer.name(db)),
    }
}
