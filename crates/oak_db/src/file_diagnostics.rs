use std::borrow::Cow;
use std::ptr;

use oak_semantic::EffectsHandlers;
use rustc_hash::FxHashSet;

use crate::diagnostic::Diagnostic;
use crate::diagnostic::DiagnosticKind;
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
/// A call is ambiguous when any sourcing context resolves it to an effect
/// different from [`File::standalone_imports()`].
pub(crate) fn inherited_shadow_diagnostics(db: &dyn Db, file: File) -> Vec<Diagnostic> {
    let contexts = file.inherited_imports_by_sourcing_file(db);
    if contexts.is_empty() {
        return Vec::new();
    }

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

        // A binding defined in this file wins in every context, so its effect
        // cannot differ.
        if !file.resolve_export(db, name).is_empty() {
            continue;
        }

        let standalone_effect = resolve_effect(db, &standalone, name.text(db).as_str());
        let clauses = sourcing_context_conflict_clauses(db, &contexts, standalone_effect, name);
        if clauses.is_empty() {
            continue;
        }

        diagnostics.push(Diagnostic::new(
            DiagnosticKind::InheritedShadow,
            format!(
                "This `{callee}` call has an ambiguous effect.\nIt resolves through {alone} when \
                 the file is sourced on its own, {clauses}.",
                alone = describe_resolution(db, &standalone, name),
                clauses = join_clauses(&clauses),
            ),
            call.range(),
            Vec::new(),
        ));
    }

    diagnostics
}

/// Builds one message clause for each sourcing context whose effect differs from
/// `standalone_effect`.
///
/// Sourcing contexts are alternative runtimes, not lookup-priority bands.
/// Combining them could let a matching context hide a differing one.
fn sourcing_context_conflict_clauses<'db>(
    db: &'db dyn Db,
    contexts: &[(File, Vec<ImportLayer>)],
    standalone_effect: Option<&'static EffectsHandlers>,
    name: Name<'db>,
) -> Vec<String> {
    let callee_text = name.text(db);
    contexts
        .iter()
        .filter(|(_, layers)| {
            !same_effect(
                resolve_effect(db, layers, callee_text.as_str()),
                standalone_effect,
            )
        })
        .map(|(sourcing, layers)| {
            format!(
                "to {resolution} when sourced by `{sourcing}`",
                resolution = describe_resolution(db, layers, name),
                sourcing = file_name(db, *sourcing),
            )
        })
        .collect()
}

fn join_clauses(clauses: &[String]) -> String {
    let mut joined = String::new();
    for (position, clause) in clauses.iter().enumerate() {
        if position > 0 {
            joined.push_str(", ");
        }
        if position + 1 == clauses.len() {
            joined.push_str("and ");
        }
        joined.push_str(clause);
    }
    joined
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

/// Describes the binding reached by looking up `name` in `layers`.
///
/// If no layer binds `name`, the lookup falls through to base builtins, which
/// resolve by name without a scanned base root.
fn describe_resolution<'db>(db: &'db dyn Db, layers: &[ImportLayer], name: Name<'db>) -> String {
    match resolve_layer(db, layers, name) {
        Some(layer) => describe_source(db, &layer),
        None => "package `base`".to_string(),
    }
}

fn describe_source(db: &dyn Db, layer: &ImportLayer) -> String {
    match layer {
        ImportLayer::File(file) | ImportLayer::SourcingFile { file, .. } => {
            format!("a binding in `{}`", file_name(db, *file))
        },
        ImportLayer::Package(package) => format!("package `{}`", package.name(db)),
        ImportLayer::From(importer) => format!("an import of `{}`", importer.name(db)),
    }
}

fn file_name(db: &dyn Db, file: File) -> Cow<'_, str> {
    file.path(db).file_name().unwrap_or_default()
}
