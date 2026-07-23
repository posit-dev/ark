use aether_parser::parse;
use aether_parser::RParserOptions;
use biome_rowan::AstNode;
use oak_semantic::build_index;
use oak_semantic::effects;
use oak_semantic::effects::SourceAnnotation;
use oak_semantic::semantic_index::DefinitionId;
use oak_semantic::semantic_index::DefinitionKind;
use oak_semantic::semantic_index::EvalEnv;
use oak_semantic::semantic_index::EvalTiming;
use oak_semantic::semantic_index::ScopeId;
use oak_semantic::semantic_index::ScopeKind;
use oak_semantic::semantic_index::SemanticCallKind;
use oak_semantic::semantic_index::SemanticDiagnostic;
use oak_semantic::semantic_index::SemanticIndex;
use oak_semantic::semantic_index::SymbolFlags;
use oak_semantic::semantic_index::UseId;
use oak_semantic::EffectsHandlers;
use oak_semantic::ImportsResolver;
use oak_semantic::NoopImportsResolver;
use oak_semantic::SourceResolution;
use url::Url;

use crate::common::index;
use crate::common::index_with_base;
use crate::common::only_assign_def;
use crate::common::semantic_call_kinds;
use crate::common::COLLATION_HANDLER;
use crate::common::MULTI_ASSIGN_HANDLER;
use crate::resolvers::TestImportsResolver;

/// Build with an arbitrary resolver, for cases that need a resolver shape the
/// shared `index`/`index_with_base` helpers don't cover.
fn build_with(source: &str, resolver: impl ImportsResolver) -> SemanticIndex {
    let parsed = parse(source, RParserOptions::default());

    if parsed.has_error() {
        panic!("source has syntax errors: {source}");
    }

    build_index(&parsed.tree(), resolver)
}

fn build_test_index(source: &str, resolver: impl ImportsResolver) -> SemanticIndex {
    let parsed = parse(source, RParserOptions::default());
    if parsed.has_error() {
        panic!("source has syntax errors: {source}");
    }
    build_index(&parsed.tree(), resolver)
}

fn helper_resolution() -> SourceResolution {
    SourceResolution {
        url: Url::parse("file:///test/helpers.R").unwrap(),
        names: vec!["helper".into()],
        packages: vec![],
    }
}

/// Returns the same resolution for any `source()` path.
struct ConstResolver(SourceResolution);

impl ImportsResolver for ConstResolver {
    fn resolve_source(&mut self, _path: &str) -> Option<SourceResolution> {
        Some(self.0.clone())
    }

    fn resolve_effects(&mut self, name: &str, _: &[String], _: bool) -> Option<EffectsHandlers> {
        // `source()` recognition runs on the resolve path, so a source-only
        // resolver still has to resolve base effects for `source` to be seen.
        effects::lookup("base", name).copied()
    }
}

/// Returns per-path resolutions; unknown paths yield `None`.
struct MapResolver(std::collections::HashMap<String, SourceResolution>);

impl ImportsResolver for MapResolver {
    fn resolve_source(&mut self, path: &str) -> Option<SourceResolution> {
        self.0.get(path).cloned()
    }

    fn resolve_effects(&mut self, name: &str, _: &[String], _: bool) -> Option<EffectsHandlers> {
        effects::lookup("base", name).copied()
    }
}

/// Resolves `source` to the multi-file [`CollationHandler`] and maps the
/// collated paths through `sources`.
struct MultiFileResolver {
    sources: std::collections::HashMap<String, SourceResolution>,
}

impl ImportsResolver for MultiFileResolver {
    fn resolve_source(&mut self, path: &str) -> Option<SourceResolution> {
        self.sources.get(path).cloned()
    }

    fn resolve_effects(&mut self, name: &str, _: &[String], _: bool) -> Option<EffectsHandlers> {
        if name == "source" {
            return Some(EffectsHandlers {
                arguments: None,
                attach: None,
                source: Some(&COLLATION_HANDLER),
                assign: None,
            });
        }
        effects::lookup("base", name).copied()
    }
}

/// Resolves `source` to a [`SourceAnnotation`] whose path sits at the second
/// positional slot, exercising the configurable `position`.
struct PositionResolver;

static SOURCE_AT_POSITION_1: SourceAnnotation = SourceAnnotation { position: 1 };

impl ImportsResolver for PositionResolver {
    fn resolve_source(&mut self, _path: &str) -> Option<SourceResolution> {
        None
    }

    fn resolve_effects(&mut self, name: &str, _: &[String], _: bool) -> Option<EffectsHandlers> {
        if name == "source" {
            return Some(EffectsHandlers {
                arguments: None,
                attach: None,
                source: Some(&SOURCE_AT_POSITION_1),
                assign: None,
            });
        }
        None
    }
}

struct MultiAssignResolver;

impl ImportsResolver for MultiAssignResolver {
    fn resolve_source(&mut self, _path: &str) -> Option<SourceResolution> {
        None
    }

    fn resolve_effects(&mut self, name: &str, _: &[String], _: bool) -> Option<EffectsHandlers> {
        if name == "assign" {
            return Some(EffectsHandlers {
                arguments: None,
                attach: None,
                source: None,
                assign: Some(&MULTI_ASSIGN_HANDLER),
            });
        }
        None
    }
}

#[test]
fn test_quote_suppresses_uses() {
    // `quote()` captures its argument unevaluated, so the symbols inside are not
    // uses. Only the callee `quote` itself is a use.
    let index = index_with_base("quote(x + y)");
    let file = ScopeId::from(0);

    assert_eq!(
        index.symbols(file).get("quote").unwrap().flags(),
        SymbolFlags::IS_USED
    );
    assert!(index.symbols(file).get("x").is_none());
    assert!(index.symbols(file).get("y").is_none());
    assert_eq!(index.uses(file).len(), 1);
}

#[test]
fn test_quote_suppresses_assignment() {
    // The `x <- 1` inside `quote()` is captured unevaluated, so it binds
    // nothing.
    let index = index_with_base("quote(x <- 1)");
    let file = ScopeId::from(0);

    assert!(index.symbols(file).get("x").is_none());
    assert_eq!(index.definitions(file).len(), 0);
}

#[test]
fn test_bquote_suppresses_uses() {
    // `bquote()` quotes its argument the same as `quote()`.
    let index = index_with_base("bquote(x + y)");
    let file = ScopeId::from(0);

    assert!(index.symbols(file).get("x").is_none());
    assert!(index.symbols(file).get("y").is_none());
    assert_eq!(index.uses(file).len(), 1);
}

#[test]
fn test_bquote_unquotes_hole() {
    // `bquote` quotes `x + .(y)`, but the `.()` hole escapes back to
    // evaluation, so `y` is a use while `x` stays quoted. `.` itself is the
    // unquote operator, not a call, so it's not a use either.
    let index = index_with_base("bquote(x + .(y))");
    let file = ScopeId::from(0);

    assert!(index.symbols(file).get("x").is_none());
    assert!(index.symbols(file).get(".").is_none());
    assert_eq!(
        index.symbols(file).get("y").unwrap().flags(),
        SymbolFlags::IS_USED
    );
}

#[test]
fn test_bquote_hole_walks_nested_expression() {
    // The `.()` content is evaluated normally, so identifiers inside it are uses
    // however deeply nested; the surrounding `f(...)` stays quoted.
    let index = index_with_base("bquote(f(.(g(z))))");
    let file = ScopeId::from(0);

    assert!(index.symbols(file).get("f").is_none());
    assert_eq!(
        index.symbols(file).get("g").unwrap().flags(),
        SymbolFlags::IS_USED
    );
    assert_eq!(
        index.symbols(file).get("z").unwrap().flags(),
        SymbolFlags::IS_USED
    );
}

#[test]
fn test_bquote_splice_true_unquotes() {
    // `..()` is bquote's splice unquote, active under `splice = TRUE`: `y` is a
    // use, `x` stays quoted.
    let index = index_with_base("bquote(x + ..(y), splice = TRUE)");
    let file = ScopeId::from(0);

    assert!(index.symbols(file).get("x").is_none());
    assert_eq!(
        index.symbols(file).get("y").unwrap().flags(),
        SymbolFlags::IS_USED
    );
}

#[test]
fn test_bquote_splice_off_by_default() {
    // `splice` defaults to FALSE, so `..()` is an ordinary quoted call and `y`
    // is not a use.
    let index = index_with_base("bquote(x + ..(y))");
    let file = ScopeId::from(0);

    assert!(index.symbols(file).get("y").is_none());
}

#[test]
fn test_bquote_splice_false_leaves_quoted() {
    // Explicit `splice = FALSE` keeps `..()` quoted.
    let index = index_with_base("bquote(..(y), splice = FALSE)");
    let file = ScopeId::from(0);

    assert!(index.symbols(file).get("y").is_none());
}

#[test]
fn test_bquote_multiple_holes() {
    // Every `.()` hole escapes, so both `a` and `b` are uses.
    let index = index_with_base("bquote(.(a) + .(b))");
    let file = ScopeId::from(0);

    assert_eq!(
        index.symbols(file).get("a").unwrap().flags(),
        SymbolFlags::IS_USED
    );
    assert_eq!(
        index.symbols(file).get("b").unwrap().flags(),
        SymbolFlags::IS_USED
    );
}

#[test]
fn test_substitute_reports_parameter_use() {
    // `substitute(x)` in a function frame substitutes the parameter `x`, so `x`
    // is a use of that binding (the `deparse(substitute(x))` idiom).
    let index = index_with_base("f <- function(x) substitute(x)");
    let fun = ScopeId::from(1);

    assert_eq!(
        index.symbols(fun).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
            .union(SymbolFlags::IS_USED)
            .union(SymbolFlags::IS_PARAMETER)
    );
}

#[test]
fn test_substitute_leaves_free_symbol_quoted() {
    // A symbol the frame doesn't bind stays quoted, so `y` is not a use while the
    // parameter `x` is.
    let index = index_with_base("f <- function(x) substitute(x + y)");
    let fun = ScopeId::from(1);

    assert!(index.symbols(fun).get("y").is_none());
    assert_eq!(
        index.symbols(fun).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
            .union(SymbolFlags::IS_USED)
            .union(SymbolFlags::IS_PARAMETER)
    );
}

#[test]
fn test_substitute_global_frame_quotes() {
    // R substitutes nothing in the global environment, so a top-level
    // `substitute` is a plain quote: `a` stays bound-only and `b` is absent. The
    // one use is the `substitute` callee itself.
    let index = index_with_base("a <- 1\nsubstitute(a + b)");
    let file = ScopeId::from(0);

    assert_eq!(
        index.symbols(file).get("a").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
    assert!(index.symbols(file).get("b").is_none());
    assert_eq!(index.uses(file).len(), 1);
}

#[test]
fn test_substitute_protects_argument_tag() {
    // The value `x` is substituted, but the `x =` tag is a name, not a symbol, so
    // it stays quoted. The two uses are the `substitute` callee and the value
    // `x`; without tag protection there would be a third for the tag.
    let index = index_with_base("f <- function(x) substitute(list(x = x))");
    let fun = ScopeId::from(1);

    assert!(index.symbols(fun).get("list").is_none());
    assert_eq!(
        index.symbols(fun).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
            .union(SymbolFlags::IS_USED)
            .union(SymbolFlags::IS_PARAMETER)
    );
    assert_eq!(index.uses(fun).len(), 2);
}

#[test]
fn test_substitute_replaces_extraction_member() {
    // Unlike ordinary evaluation, `substitute` replaces the `$` member too, so
    // the parameter `v` in `d$v` is a use while `d` stays quoted.
    let index = index_with_base("f <- function(v) substitute(d$v)");
    let fun = ScopeId::from(1);

    assert!(index.symbols(fun).get("d").is_none());
    assert_eq!(
        index.symbols(fun).get("v").unwrap().flags(),
        SymbolFlags::IS_BOUND
            .union(SymbolFlags::IS_USED)
            .union(SymbolFlags::IS_PARAMETER)
    );
}

#[test]
fn test_substitute_explicit_env_quotes_even_environment() {
    // We bail on any explicit `env`, even `environment()` (which names the frame
    // we'd otherwise query), until proper env resolution lands. So `x` stays
    // quoted rather than being reported as a use.
    let index = index_with_base("f <- function(x) substitute(x, environment())");
    let fun = ScopeId::from(1);

    assert_eq!(
        index.symbols(fun).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND.union(SymbolFlags::IS_PARAMETER)
    );
}

#[test]
fn test_substitute_non_default_env_quotes() {
    // An explicit `env` we can't see into falls back to a plain quote, so the
    // parameter `x` is not reported as a use.
    let index = index_with_base("f <- function(x) substitute(x, list())");
    let fun = ScopeId::from(1);

    assert_eq!(
        index.symbols(fun).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND.union(SymbolFlags::IS_PARAMETER)
    );
}

#[test]
fn test_substitute_local_frame() {
    // Inside `local()`, the frame is the local body, so a name the body binds is
    // substituted and reported as a use.
    let index = index_with_base("local({\n  y <- 1\n  substitute(y)\n})");
    let local = ScopeId::from(1);

    assert_eq!(
        index.symbols(local).get("y").unwrap().flags(),
        SymbolFlags::IS_BOUND.union(SymbolFlags::IS_USED)
    );
}

#[test]
fn test_substitute_quotes_nested_function_inertly() {
    // A nested function in the argument is inert language data. The frame binds
    // nothing, so no symbol inside is a use, and the function is not itself a
    // scope. It would only become a scope once the result is evaluated.
    let index = index_with_base("f <- function() substitute(function(x) x + 1)");
    let fun = ScopeId::from(1);

    assert!(index.symbols(fun).get("x").is_none());
    assert_eq!(index.child_scope_ids(fun).count(), 0);
}

#[test]
fn test_substitute_replaces_symbol_in_nested_function() {
    // `substitute` replaces symbols syntactically, ignoring the nested function's
    // own scope, so the body `x` (bound by the outer frame) is a use of the outer
    // parameter. The inner formal `x` is a tag and stays quoted, and the nested
    // function is not itself a scope. The two uses are the `substitute` callee
    // and the body `x`.
    let index = index_with_base("g <- function(x) substitute(function(x) x + 1)");
    let fun = ScopeId::from(1);

    assert_eq!(
        index.symbols(fun).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
            .union(SymbolFlags::IS_USED)
            .union(SymbolFlags::IS_PARAMETER)
    );
    assert_eq!(index.child_scope_ids(fun).count(), 0);
    assert_eq!(index.uses(fun).len(), 2);
}

#[test]
fn test_local_quote_definition_shadows() {
    // A local `quote` shadows base's, so `quote(y)` is an ordinary call and `y`
    // is a use again.
    let index = index_with_base("quote <- function(a) a\nquote(y)");
    let file = ScopeId::from(0);

    assert_eq!(
        index.symbols(file).get("y").unwrap().flags(),
        SymbolFlags::IS_USED
    );
}

#[test]
fn test_quote_suppresses_attach_effect() {
    // A `library()` inside `quote()` is captured unevaluated, so it attaches
    // nothing to the search path.
    let index = index_with_base("quote(library(dplyr))");
    assert!(semantic_call_kinds(&index).is_empty());
}

#[test]
fn test_quote_suppresses_source_effect() {
    // A `source()` inside `quote()` is captured unevaluated, so it injects no
    // names and records no source call.
    let index = index_with_base("quote(source(\"helpers.R\"))");
    assert!(semantic_call_kinds(&index).is_empty());
}

#[test]
fn test_quote_suppresses_assign_effect() {
    // An `assign()` inside `quote()` is captured unevaluated, so it binds
    // nothing.
    let index = index_with_base("quote(assign(\"x\", 1))");
    let file = ScopeId::from(0);

    assert!(index.symbols(file).get("x").is_none());
    assert_eq!(index.definitions(file).len(), 0);
}

// --- File directives ---

#[test]
fn test_directive_library_identifier() {
    let index = index_with_base("library(dplyr)");
    assert_eq!(semantic_call_kinds(&index), [&SemanticCallKind::Attach {
        package: "dplyr".into()
    }]);
}

#[test]
fn test_directive_library_string() {
    let index = index_with_base("library(\"tidyr\")");
    assert_eq!(semantic_call_kinds(&index), [&SemanticCallKind::Attach {
        package: "tidyr".into()
    }]);
}

#[test]
fn test_directive_library_single_quoted_string() {
    let index = index_with_base("library('ggplot2')");
    assert_eq!(semantic_call_kinds(&index), [&SemanticCallKind::Attach {
        package: "ggplot2".into()
    }]);
}

#[test]
fn test_directive_require() {
    let index = index_with_base("require(data.table)");
    assert_eq!(semantic_call_kinds(&index), [&SemanticCallKind::Attach {
        package: "data.table".into()
    }]);
}

#[test]
fn test_directive_multiple_libraries() {
    let index = index_with_base("library(dplyr)\nlibrary(tidyr)\nrequire(ggplot2)");
    assert_eq!(semantic_call_kinds(&index), [
        &SemanticCallKind::Attach {
            package: "dplyr".into()
        },
        &SemanticCallKind::Attach {
            package: "tidyr".into()
        },
        &SemanticCallKind::Attach {
            package: "ggplot2".into()
        },
    ]);
}

#[test]
fn test_directive_named_argument() {
    // The package binds the `package` formal by name.
    let index = index_with_base("library(package = dplyr)");
    assert_eq!(semantic_call_kinds(&index), [&SemanticCallKind::Attach {
        package: "dplyr".into()
    }]);
}

#[test]
fn test_directive_multiple_arguments() {
    // The package binds `package` positionally; the extra named argument binds
    // no formal we track.
    let index = index_with_base("library(dplyr, warn.conflicts = FALSE)");
    assert_eq!(semantic_call_kinds(&index), [&SemanticCallKind::Attach {
        package: "dplyr".into()
    }]);
}

#[test]
fn test_directive_character_only_string() {
    // `character.only = TRUE` reads the package argument with the standard rule.
    // A string literal resolves to its text.
    let index = index_with_base("library(\"dplyr\", character.only = TRUE)");
    assert_eq!(semantic_call_kinds(&index), [&SemanticCallKind::Attach {
        package: "dplyr".into()
    }]);
}

#[test]
fn test_directive_character_only_identifier_not_attached() {
    // With `character.only = TRUE` the package argument is a variable to resolve,
    // not a symbol. We can't chase it statically, so nothing is attached, rather
    // than wrongly attaching a package literally named `x`.
    let index = index_with_base("library(x, character.only = TRUE)");
    assert_eq!(semantic_call_kinds(&index), Vec::<&SemanticCallKind>::new());
}

#[test]
fn test_directive_character_only_false_is_quoted() {
    // `character.only = FALSE` leaves the package argument quoted, so the symbol
    // text is the package name.
    let index = index_with_base("library(dplyr, character.only = FALSE)");
    assert_eq!(semantic_call_kinds(&index), [&SemanticCallKind::Attach {
        package: "dplyr".into()
    }]);
}

#[test]
fn test_directive_no_arguments_ignored() {
    let index = index_with_base("library()");
    assert_eq!(semantic_call_kinds(&index), Vec::<&SemanticCallKind>::new());
}

#[test]
fn test_directive_library_in_function_scope() {
    // library() in a function body now records a scoped directive
    let index = index_with_base("f <- function() { library(dplyr) }");
    assert_eq!(semantic_call_kinds(&index), [&SemanticCallKind::Attach {
        package: "dplyr".into()
    }]);
    let semantic_calls = index.semantic_calls();
    assert_ne!(semantic_calls[0].scope(), ScopeId::from(0));
}

#[test]
fn test_directive_non_static_argument_ignored() {
    let index = index_with_base("library(get_pkg())");
    assert_eq!(semantic_call_kinds(&index), Vec::<&SemanticCallKind>::new());
}

#[test]
fn test_directive_preserves_offset() {
    let index = index_with_base("x <- 1\nlibrary(dplyr)");
    let semantic_calls = index.semantic_calls();
    assert_eq!(semantic_calls.len(), 1);
    assert_eq!(semantic_calls[0].offset(), biome_rowan::TextSize::from(7));
}

// --- source() semantic calls ---
//
// The no-resolver `semantic_index` (used by `oak_db`) always records
// a `Source` semantic call for every `source(...)` site, even when
// the path can't be resolved cross-file. Downstream queries in
// `oak_db` translate the path to a `Script` and inject the target's
// exports.

#[test]
fn test_source_call_records_path() {
    let index = index_with_base("source(\"helpers.R\")");
    assert_eq!(semantic_call_kinds(&index), [&SemanticCallKind::Source {
        path: "helpers.R".into(),
        resolved: None,
    }]);
}

#[test]
fn test_source_call_single_quoted_string() {
    let index = index_with_base("source('helpers.R')");
    assert_eq!(semantic_call_kinds(&index), [&SemanticCallKind::Source {
        path: "helpers.R".into(),
        resolved: None,
    }]);
}

#[test]
fn test_source_call_preserves_offset() {
    let index = index_with_base("x <- 1\nsource(\"helpers.R\")");
    let semantic_calls = index.semantic_calls();
    assert_eq!(semantic_calls.len(), 1);
    assert_eq!(semantic_calls[0].offset(), biome_rowan::TextSize::from(7));
}

#[test]
fn test_source_call_records_file_scope() {
    let index = index_with_base("source(\"helpers.R\")");
    let semantic_calls = index.semantic_calls();
    assert_eq!(semantic_calls.len(), 1);
    assert_eq!(semantic_calls[0].scope(), ScopeId::from(0));
}

#[test]
fn test_source_call_in_function_body_records_inner_scope() {
    let index = index_with_base("f <- function() { source(\"helpers.R\") }");
    let semantic_calls = index.semantic_calls();
    assert_eq!(semantic_calls.len(), 1);
    assert_eq!(semantic_calls[0].kind(), &SemanticCallKind::Source {
        path: "helpers.R".into(),
        resolved: None,
    });
    assert_ne!(semantic_calls[0].scope(), ScopeId::from(0));
}

#[test]
fn test_source_call_non_static_path_ignored() {
    let index = index_with_base("source(get_path())");
    assert_eq!(semantic_call_kinds(&index), Vec::<&SemanticCallKind>::new());
}

#[test]
fn test_source_call_non_static_local_ignored() {
    // `local = some_env()` isn't statically resolvable; we bail rather
    // than record the call.
    let index = index_with_base("source(\"helpers.R\", local = some_env())");
    assert_eq!(semantic_call_kinds(&index), Vec::<&SemanticCallKind>::new());
}

#[test]
fn test_source_call_local_true_recorded() {
    let index = index_with_base("source(\"helpers.R\", local = TRUE)");
    assert_eq!(semantic_call_kinds(&index), [&SemanticCallKind::Source {
        path: "helpers.R".into(),
        resolved: None,
    }]);
}

#[test]
fn test_source_call_shadowed_by_local_binding_not_recognized() {
    // A user-defined `source` shadows base `source`, so the call isn't a source
    // directive and injects nothing. Recognition runs on the resolve path, which
    // sees the local binding first.
    let index = index_with_base("source <- function(...) {}\nsource(\"helpers.R\")");
    assert_eq!(semantic_call_kinds(&index), Vec::<&SemanticCallKind>::new());
}

#[test]
fn test_source_and_library_calls_coexist() {
    let index = index_with_base("library(dplyr)\nsource(\"helpers.R\")\nrequire(tidyr)");
    assert_eq!(semantic_call_kinds(&index), [
        &SemanticCallKind::Attach {
            package: "dplyr".into()
        },
        &SemanticCallKind::Source {
            path: "helpers.R".into(),
            resolved: None,
        },
        &SemanticCallKind::Attach {
            package: "tidyr".into()
        },
    ]);
}

#[test]
fn test_source_call_recognized_under_base_resolver() {
    // Recognition runs on the resolve path now, so `source()` needs a resolver
    // that resolves base. With base attached but no registered source, the
    // resolver's `resolve_source` returns `None`: no `DefinitionKind::Import` is
    // injected for sourced names, but the `Source` semantic call IS recorded, so
    // downstream queries in `oak_db` can still chase the forwarding chain.
    let index = index_with_base("source(\"helpers.R\")");
    let file_scope = ScopeId::from(0);
    assert_eq!(index.definitions(file_scope).iter().count(), 0);
    assert_eq!(index.semantic_calls().len(), 1);
}

// --- assign() ---

#[test]
fn test_assign_records_definition() {
    // `assign("x", 1)` binds `x` in the current scope, and the later use of `x`
    // resolves to that definition.
    let index = index_with_base("assign(\"x\", 1)\nx");
    let file = ScopeId::from(0);

    assert!(matches!(
        only_assign_def(&index),
        Some(DefinitionKind::Assign { .. })
    ));

    // Uses: assign(0), x(1). The `x` use binds to the assign-created def.
    let map = index.use_def_map(file);
    let bindings = map.bindings_at_use(UseId::from(1));
    assert_eq!(bindings.definitions().len(), 1);
    let def = &index.definitions(file)[bindings.definitions()[0]];
    assert!(matches!(def.kind(), DefinitionKind::Assign { .. }));
}

#[test]
fn test_assign_qualified_base_call_recognized() {
    // The namespaced form resolves through `resolve_qualified_effects`.
    let index = index_with_base("base::assign(\"x\", 1)");
    assert!(matches!(
        only_assign_def(&index),
        Some(DefinitionKind::Assign { .. })
    ));
}

#[test]
fn test_delayed_assign_records_definition() {
    let index = index_with_base("delayedAssign(\"x\", expensive())");
    assert!(matches!(
        only_assign_def(&index),
        Some(DefinitionKind::Assign { .. })
    ));
}

#[test]
fn test_assign_non_literal_name_not_recorded() {
    // A dynamic name can't be pinned, so the effect is recognized but nothing
    // is recorded (same spirit as a dynamic `source()` path).
    let index = index_with_base("assign(nm, 1)");
    assert!(only_assign_def(&index).is_none());
}

#[test]
fn test_assign_explicit_envir_not_recorded() {
    // An explicit target environment binds outside the current scope, so we
    // skip it rather than record a def in the wrong place.
    let index = index_with_base("assign(\"x\", 1, envir = e)");
    assert!(only_assign_def(&index).is_none());
}

#[test]
fn test_assign_shadowed_by_local_binding_not_recognized() {
    // A user-defined `assign` shadows base `assign`, so the call binds nothing.
    let index = index_with_base("assign <- function(...) {}\nassign(\"x\", 1)");
    assert!(only_assign_def(&index).is_none());
}

#[test]
fn test_assign_value_handle_points_at_value_expression() {
    // The stored `value` handle resolves to the value argument, which is what a
    // type checker infers the binding's type from.
    let parsed = parse("assign(\"x\", 1 + 2)", RParserOptions::default());
    assert!(!parsed.has_error());
    let root = parsed.tree().syntax().clone();
    let index = build_index(&parsed.tree(), TestImportsResolver::with_base());

    let kind = only_assign_def(&index).expect("assign def");
    let DefinitionKind::Assign {
        value: Some(value), ..
    } = kind
    else {
        panic!("expected a value handle");
    };
    assert_eq!(
        value.to_node(&root).syntax().text_trimmed().to_string(),
        "1 + 2"
    );
}

#[test]
fn test_assign_without_value_has_no_value_handle() {
    // The name still binds, but there's no value argument to infer from.
    let index = index_with_base("assign(\"x\")");
    assert!(matches!(
        only_assign_def(&index),
        Some(DefinitionKind::Assign { value: None, .. })
    ));
}

// --- source() semantic calls: bail paths ---
//
// Cases where the builder can't extract a statically-resolvable
// path, so no `Source` semantic call is emitted. The valid-path
// cases live above ("source() semantic calls").

#[test]
fn test_source_call_identifier_path_ignored() {
    let index = index("source(my_file)");
    assert_eq!(semantic_call_kinds(&index), Vec::<&SemanticCallKind>::new());
}

#[test]
fn test_source_call_paste0_argument_ignored() {
    let index = index("source(paste0(\"path/\", name))");
    assert_eq!(semantic_call_kinds(&index), Vec::<&SemanticCallKind>::new());
}

#[test]
fn test_source_call_named_file_argument_ignored() {
    let index = index("source(file = \"helpers.R\")");
    assert_eq!(semantic_call_kinds(&index), Vec::<&SemanticCallKind>::new());
}

#[test]
fn test_source_call_no_arguments_ignored() {
    let index = index("source()");
    assert_eq!(semantic_call_kinds(&index), Vec::<&SemanticCallKind>::new());
}

// --- declare() directives ---

#[test]
fn test_directive_declare_source_no_resolver() {
    let index = index_with_base("declare(source(\"helpers.R\"))");
    assert_eq!(semantic_call_kinds(&index), [&SemanticCallKind::Source {
        path: "helpers.R".into(),
        resolved: None,
    }]);
}

#[test]
fn test_directive_declare_source_single_quotes_no_resolver() {
    let index = index_with_base("declare(source('utils.R'))");
    assert_eq!(semantic_call_kinds(&index), [&SemanticCallKind::Source {
        path: "utils.R".into(),
        resolved: None,
    }]);
}

#[test]
fn test_directive_tilde_declare_source_no_resolver() {
    let index = index_with_base("~declare(source(\"helpers.R\"))");
    assert_eq!(semantic_call_kinds(&index), [&SemanticCallKind::Source {
        path: "helpers.R".into(),
        resolved: None,
    }]);
}

#[test]
fn test_fixme_directive_declare_library_transparent() {
    // `declare()` is transparent: the inner `library(dplyr)` is still
    // picked up as a directive.
    // FIXME: We should declare `declare()` as a quoting function.
    let index = index_with_base("declare(library(dplyr))");
    assert_eq!(semantic_call_kinds(&index), [&SemanticCallKind::Attach {
        package: "dplyr".into()
    }]);
}

#[test]
fn test_directive_declare_not_at_file_scope() {
    // declare()'s argument is walked into regardless of position, so a
    // nested source() inside a function body is still recorded.
    let index = index_with_base("f <- function() { declare(source(\"helpers.R\")) }");
    assert_eq!(semantic_call_kinds(&index), [&SemanticCallKind::Source {
        path: "helpers.R".into(),
        resolved: None,
    }]);
}

#[test]
fn test_directive_tilde_declare_not_at_file_scope() {
    let index = index_with_base("f <- function() { ~declare(source(\"helpers.R\")) }");
    assert_eq!(semantic_call_kinds(&index), [&SemanticCallKind::Source {
        path: "helpers.R".into(),
        resolved: None,
    }]);
}

#[test]
fn test_directive_declare_mixed_with_bare() {
    let index =
        index_with_base("library(dplyr)\ndeclare(source(\"helpers.R\"))\nsource(\"utils.R\")");
    assert_eq!(semantic_call_kinds(&index), [
        &SemanticCallKind::Attach {
            package: "dplyr".into()
        },
        &SemanticCallKind::Source {
            path: "helpers.R".into(),
            resolved: None,
        },
        &SemanticCallKind::Source {
            path: "utils.R".into(),
            resolved: None,
        },
    ]);
}

#[test]
fn test_directive_declare_source_no_resolver_records_call() {
    let index = index_with_base("x <- 1\ndeclare(source(\"helpers.R\"))");
    let semantic_calls = index.semantic_calls();
    assert_eq!(semantic_calls.len(), 1);
    assert_eq!(semantic_calls[0].kind(), &SemanticCallKind::Source {
        path: "helpers.R".into(),
        resolved: None,
    });
}

#[test]
fn test_directive_tilde_declare_source_no_resolver_records_call() {
    let index = index_with_base("x <- 1\n~declare(source(\"helpers.R\"))");
    let semantic_calls = index.semantic_calls();
    assert_eq!(semantic_calls.len(), 1);
    assert_eq!(semantic_calls[0].kind(), &SemanticCallKind::Source {
        path: "helpers.R".into(),
        resolved: None,
    });
}

#[test]
fn test_directive_declare_non_call_arg_ignored() {
    let index = index("declare(42)");
    assert_eq!(semantic_call_kinds(&index), Vec::<&SemanticCallKind>::new());
}

#[test]
fn test_directive_declare_identifier_source_arg_ignored() {
    let index = index("declare(source(my_file))");
    assert_eq!(semantic_call_kinds(&index), Vec::<&SemanticCallKind>::new());
}

// --- source() with resolver ---

#[test]
fn test_source_resolver_injects_definitions() {
    // At file scope, source() injects Import definitions into the use-def map.
    let code = "source(\"helpers.R\")\nhelper\n";
    let index = build_test_index(code, ConstResolver(helper_resolution()));
    let file = ScopeId::from(0);

    // Use 0 is `source`, use 1 is `helper`
    let map = index.use_def_map(file);
    let bindings = map.bindings_at_use(UseId::from(1));
    assert!(!bindings.definitions().is_empty());

    let def_id = bindings.definitions()[0];
    let def = &index.definitions(file)[def_id];
    assert!(matches!(def.kind(), DefinitionKind::Import { .. }));
    match def.kind() {
        DefinitionKind::Import { file, name, .. } => {
            assert_eq!(file.as_str(), "file:///test/helpers.R");
            assert_eq!(name, "helper");
        },
        _ => panic!("expected Import kind"),
    }

    // file_exports() includes Import-kind definitions
    let exports = index.exports();
    assert!(exports.iter().any(|(name, _)| *name == "helper"));
}

#[test]
fn test_source_resolver_offset_visibility() {
    let code = "helper\nsource(\"helpers.R\")\nhelper\n";
    let index = build_test_index(code, ConstResolver(helper_resolution()));
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // First `helper` (before source call) is unbound
    let first = map.bindings_at_use(UseId::from(0));
    assert!(first.may_be_unbound());

    // Second `helper` (after source call) resolves to the sourced definition
    // Uses: helper(0), source(1), helper(2)
    let second = map.bindings_at_use(UseId::from(2));
    assert!(!second.definitions().is_empty());
    let def_id = second.definitions()[0];
    let def = &index.definitions(file)[def_id];
    assert!(matches!(def.kind(), DefinitionKind::Import { .. }));
}

#[test]
fn test_source_resolver_in_function_scope() {
    // source() in a function scope injects Import-kind defs into
    // the function scope's use-def map.
    let code = "f <- function() {\n  source(\"helpers.R\")\n  helper\n}\nhelper\n";
    let index = build_test_index(code, ConstResolver(helper_resolution()));
    let fun = ScopeId::from(1);
    let file = ScopeId::from(0);

    // Function scope: source(0), helper(1)
    let fun_map = index.use_def_map(fun);
    let inner_bindings = fun_map.bindings_at_use(UseId::from(1));
    assert_eq!(inner_bindings.definitions().len(), 1);
    let def = &index.definitions(fun)[inner_bindings.definitions()[0]];
    assert!(matches!(def.kind(), DefinitionKind::Import { .. }));

    // File scope: `helper` does not resolve
    let file_map = index.use_def_map(file);
    let outer_bindings = file_map.bindings_at_use(UseId::from(0));
    assert!(outer_bindings.definitions().is_empty());
    assert!(outer_bindings.may_be_unbound());
}

#[test]
fn test_source_resolver_packages_become_attach_calls() {
    // The source() call is always recorded as a `Source` semantic call.
    // With a resolver, packages attached transitively by the sourced
    // file are *additionally* recorded as `Attach` semantic calls (the
    // legacy "library() in a sourced file propagates to caller" path).
    let code = "source(\"helpers.R\")\n";
    let index = build_test_index(
        code,
        ConstResolver(SourceResolution {
            url: Url::parse("file:///test/helpers.R").unwrap(),
            names: vec![],
            packages: vec!["dplyr".into()],
        }),
    );

    assert_eq!(semantic_call_kinds(&index), [
        &SemanticCallKind::Source {
            path: "helpers.R".into(),
            resolved: Some(Url::parse("file:///test/helpers.R").unwrap()),
        },
        &SemanticCallKind::Attach {
            package: "dplyr".into()
        },
    ]);
}

#[test]
fn test_source_resolver_later_shadows_earlier() {
    // At file scope, both source() calls inject Import definitions
    // into the use-def map. The later one shadows the earlier.
    let code = "source(\"a.R\")\nsource(\"b.R\")\nfoo\n";

    let a_url = Url::parse("file:///test/a.R").unwrap();
    let b_url = Url::parse("file:///test/b.R").unwrap();

    let mut resolutions = std::collections::HashMap::new();
    resolutions.insert("a.R".to_string(), SourceResolution {
        url: a_url.clone(),
        names: vec!["foo".into()],
        packages: Vec::new(),
    });
    resolutions.insert("b.R".to_string(), SourceResolution {
        url: b_url.clone(),
        names: vec!["foo".into()],
        packages: Vec::new(),
    });

    let index = build_test_index(code, MapResolver(resolutions));

    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // Uses: source(0), source(1), foo(2)
    let bindings = map.bindings_at_use(UseId::from(2));
    assert_eq!(bindings.definitions().len(), 1);

    let def_id = bindings.definitions()[0];
    let def = &index.definitions(file)[def_id];
    assert!(matches!(def.kind(), DefinitionKind::Import { .. }));
    match def.kind() {
        DefinitionKind::Import { file, .. } => assert_eq!(*file, b_url),
        _ => panic!("expected Import kind"),
    }
}

#[test]
fn test_source_resolver_local_true_in_function_scope() {
    // `local = TRUE` injects Import definitions into the function
    // scope's use-def map.
    let code = "f <- function() {\n  source(\"helpers.R\", local = TRUE)\n  helper\n}\nhelper\n";
    let index = build_test_index(code, ConstResolver(helper_resolution()));
    let fun = ScopeId::from(1);
    let file = ScopeId::from(0);

    let fun_map = index.use_def_map(fun);
    // Function scope uses: source(0), helper(1)
    let inner_bindings = fun_map.bindings_at_use(UseId::from(1));
    assert_eq!(inner_bindings.definitions().len(), 1);
    let def = &index.definitions(fun)[inner_bindings.definitions()[0]];
    assert!(matches!(def.kind(), DefinitionKind::Import { .. }));

    // File scope: `helper` does not resolve
    let file_map = index.use_def_map(file);
    let outer_bindings = file_map.bindings_at_use(UseId::from(0));
    assert!(outer_bindings.definitions().is_empty());
}

#[test]
fn test_source_resolver_local_true_shadows_local_def() {
    // `source(local = TRUE)` injects into the use-def map and
    // shadows a prior local binding.
    let code = "f <- function() {\n  foo <- 1\n  source(\"helpers.R\", local = TRUE)\n  foo\n}\n";
    let index = build_test_index(
        code,
        ConstResolver(SourceResolution {
            url: Url::parse("file:///test/helpers.R").unwrap(),
            names: vec!["foo".into()],
            packages: vec![],
        }),
    );
    let fun = ScopeId::from(1);

    let fun_map = index.use_def_map(fun);
    // Function scope uses: source(0), foo(1)
    let bindings = fun_map.bindings_at_use(UseId::from(1));
    assert_eq!(bindings.definitions().len(), 1);
    let def = &index.definitions(fun)[bindings.definitions()[0]];
    assert!(matches!(def.kind(), DefinitionKind::Import { .. }));
}

#[test]
fn test_source_resolver_local_false_does_not_shadow_local_def() {
    // source() without `local = TRUE` in a function scope now also
    // injects Import definitions, shadowing the local binding.
    let code = "f <- function() {\n  foo <- 1\n  source(\"helpers.R\")\n  foo\n}\n";
    let index = build_test_index(
        code,
        ConstResolver(SourceResolution {
            url: Url::parse("file:///test/helpers.R").unwrap(),
            names: vec!["foo".into()],
            packages: vec![],
        }),
    );
    let fun = ScopeId::from(1);

    let fun_map = index.use_def_map(fun);
    // Function scope uses: source(0), foo(1)
    let bindings = fun_map.bindings_at_use(UseId::from(1));
    assert_eq!(bindings.definitions().len(), 1);
    let def = &index.definitions(fun)[bindings.definitions()[0]];
    assert!(matches!(def.kind(), DefinitionKind::Import { .. }));
}

#[test]
fn test_source_resolver_local_def_shadowed_by_source() {
    // A local definition followed by source() at file scope:
    // the source() shadows the local def.
    let code = "helper <- 1\nsource(\"helpers.R\")\nhelper\n";
    let index = build_test_index(code, ConstResolver(helper_resolution()));
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // Uses: source(0), helper(1)
    let bindings = map.bindings_at_use(UseId::from(1));
    assert_eq!(bindings.definitions().len(), 1);
    let def_id = bindings.definitions()[0];
    let def = &index.definitions(file)[def_id];
    assert!(matches!(def.kind(), DefinitionKind::Import { .. }));
}

#[test]
fn test_source_resolver_multiple_files_each_emitted_and_injected() {
    // A source handler can resolve one call to several files (a collation).
    // Each file becomes its own `Source` semantic call and injects its own
    // names, in file order, with each file's forwarded packages after it.
    let sources = std::collections::HashMap::from([
        ("a.R".to_string(), SourceResolution {
            url: Url::parse("file:///a.R").unwrap(),
            names: vec!["a_name".into()],
            packages: vec!["pkgA".into()],
        }),
        ("b.R".to_string(), SourceResolution {
            url: Url::parse("file:///b.R").unwrap(),
            names: vec!["b_name".into()],
            packages: vec![],
        }),
    ]);
    let code = "source(\"collate\")\na_name\nb_name\n";
    let index = build_test_index(code, MultiFileResolver { sources });

    assert_eq!(semantic_call_kinds(&index), [
        &SemanticCallKind::Source {
            path: "a.R".into(),
            resolved: Some(Url::parse("file:///a.R").unwrap()),
        },
        &SemanticCallKind::Attach {
            package: "pkgA".into()
        },
        &SemanticCallKind::Source {
            path: "b.R".into(),
            resolved: Some(Url::parse("file:///b.R").unwrap()),
        },
    ]);

    // Both files' names are injected and resolve at their uses.
    // Uses: source(0), a_name(1), b_name(2)
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);
    for use_index in [1, 2] {
        let bindings = map.bindings_at_use(UseId::from(use_index));
        assert_eq!(bindings.definitions().len(), 1);
        let def = &index.definitions(file)[bindings.definitions()[0]];
        assert!(matches!(def.kind(), DefinitionKind::Import { .. }));
    }
}

#[test]
fn test_source_resolver_honors_configured_path_position() {
    // A `SourceAnnotation` with `position: 1` takes the path from the second
    // positional argument, not the first.
    let index = build_test_index("source(\"ignored\", \"real.R\")", PositionResolver);
    assert_eq!(semantic_call_kinds(&index), [&SemanticCallKind::Source {
        path: "real.R".into(),
        resolved: None,
    }]);
}

#[test]
fn test_source_call_leading_named_arg_still_finds_path() {
    // A named argument before the path doesn't consume the positional slot, so
    // the path is still recognized (unlike full call-position matching).
    let index = index_with_base("source(echo = TRUE, \"helpers.R\")");
    assert_eq!(semantic_call_kinds(&index), [&SemanticCallKind::Source {
        path: "helpers.R".into(),
        resolved: None,
    }]);
}

#[test]
fn test_assign_resolver_multiple_names_each_defined() {
    // An assign handler can bind several names from one call. Each becomes its
    // own definition and resolves at its use.
    let code = "assign(\"unused\")\na\nb\n";
    let index = build_test_index(code, MultiAssignResolver);
    let file = ScopeId::from(0);

    let assign_defs = index
        .definitions(file)
        .iter()
        .filter(|(_, def)| matches!(def.kind(), DefinitionKind::Assign { .. }))
        .count();
    assert_eq!(assign_defs, 2);

    // Uses: assign(0), a(1), b(2). Both resolve to an assign-created def.
    let map = index.use_def_map(file);
    for use_index in [1, 2] {
        let bindings = map.bindings_at_use(UseId::from(use_index));
        assert_eq!(bindings.definitions().len(), 1);
        let def = &index.definitions(file)[bindings.definitions()[0]];
        assert!(matches!(def.kind(), DefinitionKind::Assign { .. }));
    }
}
// --- NSE scopes ---

#[test]
fn test_nse_local_creates_nested_eager_scope() {
    let index = index_with_base(
        "\
local({
    x <- 1
})
",
    );
    let file = ScopeId::from(0);
    let local_scope = ScopeId::from(1);

    // `local` is used at file scope
    assert_eq!(index.symbols(file).len(), 1);
    assert_eq!(
        index.symbols(file).get("local").unwrap().flags(),
        SymbolFlags::IS_USED
    );

    // `x` is defined inside the NSE scope, not at file level
    assert_eq!(
        index.scope(local_scope).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
    );
    assert_eq!(index.scope(local_scope).parent(), Some(file));
    assert_eq!(index.symbols(local_scope).len(), 1);
    assert_eq!(
        index.symbols(local_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_local_definition_not_in_parent() {
    // Definitions inside `local()` should NOT leak to the file scope.
    let index = index_with_base(
        "\
local({
    x <- 1
})
x
",
    );
    let file = ScopeId::from(0);

    // `x` at file scope is only IS_USED (from the bare `x` on the last line),
    // not IS_BOUND (from the assignment inside local).
    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_USED);
}

#[test]
fn test_nse_evalq_no_scope_push() {
    // `evalq` is Current + Eager: no scope push, walk body in place.
    let index = index_with_base(
        "\
evalq({
    x <- 1
})
",
    );
    let file = ScopeId::from(0);

    // Only the file scope exists (plus no child scopes)
    assert_eq!(
        index.symbols(file).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
    // evalq is used, x is bound
    assert_eq!(index.symbols(file).len(), 2);
}

#[test]
fn test_nse_shadowed_name_no_scope() {
    // If `local` is locally defined (shadowed), it's not recognized as NSE.
    let index = index_with_base(
        "\
local <- identity
local({
    x <- 1
})
",
    );
    let file = ScopeId::from(0);

    // `local` is defined at file scope, shadowing the base function.
    // No NSE scope should be created. `x` is defined at file scope.
    assert_eq!(
        index.symbols(file).get("local").unwrap().flags(),
        SymbolFlags::IS_BOUND.union(SymbolFlags::IS_USED)
    );
    assert_eq!(
        index.symbols(file).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_ancestor_shadowed_name_no_scope() {
    // A `local` binding in an ENCLOSING scope shadows the base function too,
    // even when the call site sits in a nested scope where `local` is free.
    let index = index_with_base(
        "\
local <- function(x) x
f <- function() {
    local({
        y <- 1
    })
}
",
    );
    let file = ScopeId::from(0);
    let identity_fn = ScopeId::from(1);
    let f_scope = ScopeId::from(2);

    // Only three scopes: no NSE scope is pushed for the shadowed `local()`.
    assert_eq!(index.scope_ids().count(), 3);
    assert_eq!(index.scope(file).kind(), ScopeKind::File);
    assert_eq!(index.scope(identity_fn).kind(), ScopeKind::Function);
    assert_eq!(index.scope(f_scope).kind(), ScopeKind::Function);

    // `local` is bound at file scope.
    assert!(index
        .symbols(file)
        .get("local")
        .unwrap()
        .flags()
        .contains(SymbolFlags::IS_BOUND));

    // `y` is defined flat in `f`, not moved into an NSE child scope.
    assert_eq!(
        index.symbols(f_scope).get("y").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_forward_def_visible_to_nested_function() {
    // A function defined inside an eager NSE body that references a name bound
    // LATER in that same body must still resolve to the NSE scope. This relies
    // on the NSE scope's own pre-scan seeing the forward definition, which the
    // pre-scan must collect despite the body range being a Nested NSE range.
    let index = index_with_base(
        "\
local({
    f <- function() x
    x <- 1
})
",
    );
    let local_scope = ScopeId::from(1);
    let f_scope = ScopeId::from(2);

    assert_eq!(
        index.scope(local_scope).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
    );
    assert_eq!(index.scope(f_scope).kind(), ScopeKind::Function);

    // `x` inside `f` resolves to the `local` scope (not the file scope), and
    // its lazy snapshot picks up `x <- 1` (DefinitionId 1 in the local scope:
    // `f` is DefinitionId 0, `x` is DefinitionId 1).
    let (enclosing_scope, bindings) = index.enclosing_bindings(f_scope, UseId::from(0)).unwrap();
    assert_eq!(enclosing_scope, local_scope);
    assert_eq!(bindings.definitions(), &[DefinitionId::from(1)]);
}

#[test]
fn test_nse_moves_definitions_into_nested_scope() {
    // Definitions inside an NSE body land in the NSE child scope, not the
    // parent, even with sibling definitions on either side at file level.
    let index = index_with_base(
        "\
x <- 0
local({
    y <- 1
})
z <- 2
",
    );
    let file = ScopeId::from(0);
    let local_scope = ScopeId::from(1);

    // File scope: x, z are bound; local is used; y is NOT in file scope
    assert!(index.symbols(file).get("x").is_some());
    assert!(index.symbols(file).get("z").is_some());
    assert!(index.symbols(file).get("local").is_some());
    assert!(index.symbols(file).get("y").is_none());

    // local scope: y is bound
    assert_eq!(
        index.symbols(local_scope).get("y").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_nested_function_inside_local() {
    // A function defined inside `local()` creates a nested Function scope.
    let index = index_with_base(
        "\
local({
    f <- function(x) x
})
",
    );
    let local_scope = ScopeId::from(1);
    let fun_scope = ScopeId::from(2);

    assert_eq!(
        index.scope(local_scope).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
    );
    assert_eq!(index.scope(fun_scope).kind(), ScopeKind::Function);
    assert_eq!(index.scope(fun_scope).parent(), Some(local_scope));
}

#[test]
fn test_nse_prescan_skips_nested_bodies() {
    // The file scope's bound names must NOT include definitions from inside
    // `local()` bodies. This means a function defined AFTER the local() call
    // should not see `x` from inside local via the bound names.
    let index = index_with_base(
        "\
local({
    x <- 1
})
f <- function() x
",
    );
    let file = ScopeId::from(0);
    let fun_scope = ScopeId::from(2);

    // `x` should NOT be in the file scope
    assert!(index.symbols(file).get("x").is_none());

    // In `f`, `x` is free and unbound -- no enclosing snapshot should find it
    // in the file scope.
    assert_eq!(index.enclosing_bindings(fun_scope, UseId::from(0)), None);
}

#[test]
fn test_nse_eager_snapshot_precise() {
    // Eager NSE scope at file level should see a point-in-time snapshot:
    // only definitions that precede the call site, not later ones.
    let index = index_with_base(
        "\
x <- 1
local({
    x
})
x <- 2
",
    );
    let local_scope = ScopeId::from(1);

    // `x` inside local is free. Its enclosing snapshot should be eager
    // (point-in-time). At the call site, only `x <- 1` (DefinitionId 0) is
    // live. `x <- 2` (DefinitionId 2) comes after and should NOT be included.
    let (enclosing_scope, bindings) = index
        .enclosing_bindings(local_scope, UseId::from(0))
        .unwrap();
    assert_eq!(enclosing_scope, ScopeId::from(0));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(0)]);
    assert!(!bindings.may_be_unbound());
}

#[test]
fn test_nse_unmasked_call_via_nested_scope() {
    // Redefining `local` inside a `local()` body doesn't shadow a later
    // `local()` call: the rebind lands in the first body's NSE scope, so it
    // never enters the file's bound names. The scan walks the first `local()`
    // inline, so the second call sees `local` still unbound in the same pass.
    let index = index_with_base(
        "\
local({
    local <- identity
})
local({
    x <- 1
})
",
    );
    let file = ScopeId::from(0);
    let first_local = ScopeId::from(1);
    let second_local = ScopeId::from(2);

    // Both calls create Nested + Eager scopes
    assert_eq!(
        index.scope(first_local).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
    );
    assert_eq!(
        index.scope(second_local).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
    );

    // `local <- identity` is in the first scope, not at file level
    assert!(index
        .symbols(file)
        .get("local")
        .unwrap()
        .flags()
        .contains(SymbolFlags::IS_USED));
    assert!(!index
        .symbols(file)
        .get("local")
        .unwrap()
        .flags()
        .contains(SymbolFlags::IS_BOUND));

    // `x <- 1` is in the second scope, not at file level
    assert!(index.symbols(file).get("x").is_none());
    assert_eq!(
        index.symbols(second_local).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_ancestor_unmask_across_function() {
    // The `local <- identity` rebind lives inside the outer `local()` body, so
    // it never enters the file's bound names. The scan of `f`'s body therefore
    // sees base `local` unbound and marks the inner `local()` NSE, cutting its
    // body out of `f`'s bound names in the same pass. So `x <- 1` lands in the
    // inner NSE scope and `g`'s free `x` stays unresolved (the sibling
    // `local()` binds `x` in its own env, invisible to `g`). The old re-walk
    // needed a second iteration to reach this; the scan gets it in one.
    let index = index_with_base(
        "\
local({
    local <- identity
})
f <- function() {
    g <- function() x
    local({
        x <- 1
    })
}
",
    );
    let file = ScopeId::from(0);
    let outer_local = ScopeId::from(1);
    let f_scope = ScopeId::from(2);
    let g_scope = ScopeId::from(3);
    let inner_local = ScopeId::from(4);

    assert_eq!(index.scope_ids().count(), 5);
    assert_eq!(
        index.scope(outer_local).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
    );
    assert_eq!(index.scope(outer_local).parent(), Some(file));
    assert_eq!(index.scope(f_scope).kind(), ScopeKind::Function);
    assert_eq!(index.scope(g_scope).kind(), ScopeKind::Function);
    assert_eq!(index.scope(g_scope).parent(), Some(f_scope));
    assert_eq!(
        index.scope(inner_local).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
    );
    assert_eq!(index.scope(inner_local).parent(), Some(f_scope));

    // `x <- 1` lands in the inner local scope, not in `f`.
    assert!(index.symbols(f_scope).get("x").is_none());
    assert_eq!(
        index.symbols(inner_local).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );

    // `g`'s free `x` resolves to nothing: the sibling `local()` binds `x` in
    // its own scope, not in `f`. Flow-insensitive bound names would wrongly
    // point it at a stray `x` in `f`.
    assert_eq!(index.enclosing_bindings(g_scope, UseId::from(0)), None);
}

#[test]
fn test_nse_sibling_branch_flow_precise() {
    // Flow-precise scan across `if`/`else`. `local` is bound only on the
    // consequence path, so on the else path base `local` is still unbound and
    // `local({...})` is NSE. Flow-insensitive bound names would see `local`
    // bound (from the consequence) and miss the NSE call, leaking `y` into the
    // file scope.
    let index = index_with_base(
        "\
if (c) local <- identity else local({
    y <- 1
})
",
    );
    let file = ScopeId::from(0);
    let nse_scope = ScopeId::from(1);

    // Only the file scope and the else branch's NSE scope exist.
    assert_eq!(index.scope_ids().count(), 2);
    assert_eq!(
        index.scope(nse_scope).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
    );
    assert_eq!(index.scope(nse_scope).parent(), Some(file));

    // `y` lands in the NSE scope, not the file scope.
    assert!(index.symbols(file).get("y").is_none());
    assert_eq!(
        index.symbols(nse_scope).get("y").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );

    // `local` is bound at file scope (from the consequence branch).
    assert!(index
        .symbols(file)
        .get("local")
        .unwrap()
        .flags()
        .contains(SymbolFlags::IS_BOUND));
}

#[test]
fn test_nse_eager_lazy_split_on_later_binding() {
    // A later file-level `local <- identity` does NOT shadow `local()` inside the
    // function `f`. `f`'s body is lazy, so its run time relative to the binding
    // is unknown, and `is_locally_bound` reads only `f`'s eager predecessors (the
    // predecessor snapshot, empty here). So the lazy `local()` is optimistically
    // NSE and `x` moves into its own scope. The genuine ambiguity (does `f` run
    // before or after the binding?) is the overturn lint's job, not a shadow.
    //
    // The eager `local()` at file scope is NSE too, but for a determined reason:
    // it runs before the binding, so its flow-precise state has `local` unbound.
    let index = index_with_base(
        "\
f <- function() {
    local({
        x <- 1
    })
}
local({
    y <- 1
})
local <- identity
",
    );
    let file = ScopeId::from(0);
    let f_scope = ScopeId::from(1);
    let f_local = ScopeId::from(2);
    let eager_local = ScopeId::from(3);

    // Four scopes: file, `f`, the NSE `local()` in `f`, and the eager `local()`.
    assert_eq!(index.scope_ids().count(), 4);
    assert_eq!(index.scope(f_scope).kind(), ScopeKind::Function);

    // Lazy `local()` in `f` is NSE (later binding is not a predecessor), so `x`
    // moves into its own scope.
    assert_eq!(
        index.scope(f_local).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
    );
    assert_eq!(index.scope(f_local).parent(), Some(f_scope));
    assert!(index.symbols(f_scope).get("x").is_none());
    assert_eq!(
        index.symbols(f_local).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );

    // Eager file-level `local()` runs before the binding, so it is NSE and `y`
    // lands in its own scope.
    assert_eq!(
        index.scope(eager_local).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
    );
    assert_eq!(index.scope(eager_local).parent(), Some(file));
    assert!(index.symbols(f_scope).get("y").is_none());
    assert_eq!(
        index.symbols(eager_local).get("y").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_local_inside_function() {
    // `local()` inside a function: the function boundary is lazy, so the
    // eager snapshot precision of `local` is bounded by the function's
    // laziness. Free variables in `local` resolve through the function's
    // lazy snapshot.
    let index = index_with_base(
        "\
x <- 1
f <- function() {
    local({
        x
    })
}
x <- 2
",
    );
    let fun_scope = ScopeId::from(1);
    let local_scope = ScopeId::from(2);

    assert_eq!(index.scope(fun_scope).kind(), ScopeKind::Function);
    assert_eq!(
        index.scope(local_scope).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
    );
    assert_eq!(index.scope(local_scope).parent(), Some(fun_scope));

    // `x` is free in the local scope, resolves through to the function scope,
    // then to the file scope. The function scope is lazy, so both defs are
    // visible despite `local` being eager.
    let (enclosing_scope, bindings) = index
        .enclosing_bindings(local_scope, UseId::from(0))
        .unwrap();
    assert_eq!(enclosing_scope, ScopeId::from(0));
    assert_eq!(bindings.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(2)
    ]);
}

#[test]
fn test_nse_nested_local_scopes() {
    // Nested `local()` inside `local()`: both create child scopes.
    let index = index_with_base(
        "\
local({
    x <- 1
    local({
        y <- 2
    })
})
",
    );
    let file = ScopeId::from(0);
    let outer_local = ScopeId::from(1);
    let inner_local = ScopeId::from(2);

    assert_eq!(
        index.scope(outer_local).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
    );
    assert_eq!(index.scope(outer_local).parent(), Some(file));
    assert_eq!(
        index.scope(inner_local).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
    );
    assert_eq!(index.scope(inner_local).parent(), Some(outer_local));

    // Definitions land in their respective scopes
    assert!(index.symbols(file).get("x").is_none());
    assert!(index.symbols(file).get("y").is_none());
    assert_eq!(
        index.symbols(outer_local).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
    assert!(index.symbols(outer_local).get("y").is_none());
    assert_eq!(
        index.symbols(inner_local).get("y").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_super_assignment_inside_local() {
    // `<<-` inside `local()` should target the grandparent (file scope),
    // not the local scope itself.
    let index = index_with_base(
        "\
x <- 1
local({
    x <<- 2
})
",
    );
    let file = ScopeId::from(0);
    let local_scope = ScopeId::from(1);

    // `x` is bound at file scope (from `x <- 1` and the `<<-`)
    assert!(index
        .symbols(file)
        .get("x")
        .unwrap()
        .flags()
        .contains(SymbolFlags::IS_BOUND));

    // `x` in the local scope is IS_SUPER_BOUND (the `<<-` site)
    assert!(index
        .symbols(local_scope)
        .get("x")
        .unwrap()
        .flags()
        .contains(SymbolFlags::IS_SUPER_BOUND));
}

#[test]
fn test_nse_eager_super_assignment_visible_to_later_use() {
    // A `<<-` inside an eager NSE body mutates the enclosing binding mid-run.
    // Each use captures its own point-in-time snapshot, so the use before the
    // `<<-` sees only `x <- 1` while the use after it also sees the `<<-`.
    let index = index_with_base(
        "\
x <- 1
local({
    x
    x <<- 2
    x
})
",
    );
    let file = ScopeId::from(0);
    let local_scope = ScopeId::from(1);

    // Use 0 is before the `<<-`: only `x <- 1` (DefinitionId 0).
    let (before_scope, before) = index
        .enclosing_bindings(local_scope, UseId::from(0))
        .unwrap();
    assert_eq!(before_scope, file);
    assert_eq!(before.definitions(), &[DefinitionId::from(0)]);

    // Use 1 is after the `<<-`: `x <- 1` (0) and the `<<-` target (1).
    let (after_scope, after) = index
        .enclosing_bindings(local_scope, UseId::from(1))
        .unwrap();
    assert_eq!(after_scope, file);
    assert_eq!(after.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(1)
    ]);
}

#[test]
fn test_nse_eager_snapshot_excludes_unrelated_super_assignment() {
    // The eager snapshot is point-in-time with no watcher, so a `<<-` in a
    // function defined after the `local()` call can't fold into it. `f`'s body
    // runs at an unknown later time and can't reach the already-run eager body.
    let index = index_with_base(
        "\
x <- 1
local({
    x
})
f <- function() {
    x <<- 2
}
",
    );
    let file = ScopeId::from(0);
    let local_scope = ScopeId::from(1);

    // Only `x <- 1` (DefinitionId 0). The `<<-` target recorded later while f's
    // body is walked is excluded, since the eager snapshot took no watcher.
    let (enclosing_scope, bindings) = index
        .enclosing_bindings(local_scope, UseId::from(0))
        .unwrap();
    assert_eq!(enclosing_scope, file);
    assert_eq!(bindings.definitions(), &[DefinitionId::from(0)]);
}

#[test]
fn test_nse_eager_snapshot_excludes_unrelated_routed_definition() {
    // `on_load` is `Current + Lazy`, so its `x <- 2` routes to the file scope
    // as a deferred def recorded after `local()`. The eager snapshot takes no
    // watcher, so the routed def is excluded, correct since it can't reach the
    // already-run body.
    let index = index_with_base(
        "\
x <- 1
local({
    x
})
rlang::on_load({
    x <- 2
})
",
    );
    let file = ScopeId::from(0);
    let local_scope = ScopeId::from(1);

    // Only `x <- 1` (DefinitionId 0). The routed `x <- 2` is excluded.
    let (enclosing_scope, bindings) = index
        .enclosing_bindings(local_scope, UseId::from(0))
        .unwrap();
    assert_eq!(enclosing_scope, file);
    assert_eq!(bindings.definitions(), &[DefinitionId::from(0)]);
}

// --- Resolver-driven recognition ---

#[test]
fn test_nse_noop_resolver_bare_local_stays_flat() {
    // Under Noop, `resolve_effects` returns `None`, so a bare `local` isn't
    // recognized as NSE: no scope is pushed and `x` stays at file scope.
    let index = build_with(
        "\
local({
    x <- 1
})
",
        NoopImportsResolver,
    );
    let file = ScopeId::from(0);

    assert_eq!(index.scope_ids().count(), 1);
    assert_eq!(
        index.symbols(file).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_noop_resolver_namespaced_local_pushes_scope() {
    // `pkg::fn` resolves through the resolver's default `resolve_qualified_effects`,
    // which reads the static registry. `::` names the package, so there's no
    // shadowing and no cross-file context needed, hence `base::local` is
    // recognized as NSE even under Noop.
    let index = build_with(
        "\
base::local({
    x <- 1
})
",
        NoopImportsResolver,
    );
    let local_scope = ScopeId::from(1);

    assert_eq!(
        index.scope(local_scope).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
    );
    assert_eq!(
        index.symbols(local_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_front_gate_skips_resolver_for_unannotated_name() {
    // A bare callee whose name no package annotates never reaches the resolver:
    // the `annotates()` gate short-circuits before consultation.
    let resolver = TestImportsResolver::with_base();
    let consultations = resolver.consultations();

    build_with("frobnicate({ x <- 1 })", resolver);

    assert_eq!(consultations.get(), 0);
}

#[test]
fn test_nse_front_gate_consults_resolver_for_annotated_name() {
    // An annotated bare callee does reach the resolver (contrast with the gate
    // test above).
    let resolver = TestImportsResolver::with_base();
    let consultations = resolver.consultations();

    build_with("local({ x <- 1 })", resolver);

    assert!(consultations.get() > 0);
}

// --- source() bindings visible to the scan ---

#[test]
fn test_nse_sourced_name_shadows_base_callee() {
    // A `source()`-injected `local` shadows base `local`, so the later
    // `local({...})` is NOT NSE. The scan binds the sourced names eagerly
    // (source() runs at its position), so the later callee sees the shadow in
    // the same pass, even though the walk injects the Import def later.
    let index = build_with(
        "\
source(\"utils.R\")
local({
    x <- 1
})
",
        TestImportsResolver::with_base().with_source("utils.R", &["local"]),
    );
    let file = ScopeId::from(0);

    // No NSE scope: the sourced `local` shadows base, so `x` stays flat.
    assert_eq!(index.scope_ids().count(), 1);
    assert_eq!(
        index.symbols(file).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_sourced_file_without_name_leaves_callee_nse() {
    // Same shape, but the sourced file does not define `local`, so base
    // `local` is unshadowed and `local({...})` IS NSE.
    let index = build_with(
        "\
source(\"utils.R\")
local({
    x <- 1
})
",
        TestImportsResolver::with_base().with_source("utils.R", &["other"]),
    );
    let file = ScopeId::from(0);
    let local_scope = ScopeId::from(1);

    assert_eq!(
        index.scope(local_scope).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
    );
    assert_eq!(index.scope(local_scope).parent(), Some(file));
    assert!(index.symbols(file).get("x").is_none());
    assert_eq!(
        index.symbols(local_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_assign_shadows_base_callee_eager() {
    // `assign("local", identity)` binds `local`, so the later `local({...})` is
    // NOT NSE. The scan records the assign-created binding in flow order, so the
    // later callee sees the shadow in the same pass.
    let index = index_with_base(
        "\
assign(\"local\", identity)
local({
    x <- 1
})
",
    );
    let file = ScopeId::from(0);

    // No NSE scope: `local` is shadowed, so `x` lands at file scope.
    assert_eq!(index.scope_ids().count(), 1);
    assert_eq!(
        index.symbols(file).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_assign_shadows_base_callee_in_lazy_body() {
    // The file-scope `assign("local", ...)` must be visible to the lazy shadow
    // check when `f`'s deferred body resolves `local`. The file scan completes
    // before the walk enters `f`, so `bound_names[file]` already carries
    // `local` and the callee is correctly treated as shadowed (not NSE).
    let index = index_with_base(
        "\
assign(\"local\", identity)
f <- function() local({
    x <- 1
})
",
    );

    // Scopes: file(0) and f(1) only. A phantom NSE scope inside `f` would be a
    // third.
    assert_eq!(index.scope_ids().count(), 2);
    let f_scope = ScopeId::from(1);
    assert_eq!(
        index.symbols(f_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

// --- NSE calls in parameter defaults ---

#[test]
fn test_nse_parameter_default_pushes_scope() {
    // An NSE call in a parameter default is recognized and pushes its scope.
    let index = index_with_base("f <- function(a = local({ x <- 1 })) a\n");
    let f_scope = ScopeId::from(1);
    let local_scope = ScopeId::from(2);

    assert_eq!(index.scope(f_scope).kind(), ScopeKind::Function);
    assert_eq!(
        index.scope(local_scope).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
    );
    assert_eq!(index.scope(local_scope).parent(), Some(f_scope));

    // `x` lands in the default's NSE scope, not the function scope.
    assert!(index.symbols(f_scope).get("x").is_none());
    assert_eq!(
        index.symbols(local_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_parameter_default_shadowed_by_param() {
    // All formals bind at once, so a `local` parameter shadows base `local` in
    // a later default, regardless of order: `local({...})` is NOT NSE and `x`
    // stays flat in the function scope.
    let index = index_with_base("f <- function(local, a = local({ x <- 1 })) a\n");
    let f_scope = ScopeId::from(1);

    assert_eq!(index.scope_ids().count(), 2);
    assert_eq!(
        index.symbols(f_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

// --- Lazy shadow ambiguity diagnostics ---

#[test]
fn test_diagnostic_lazy_shadow_later_eager_binding() {
    // `f`'s `local()` is optimistically NSE, but a later file-level `local`
    // binding could shadow it depending on when `f` runs. Flagged.
    let source = "\
f <- function() local({ x <- 1 })
local <- identity
";
    let index = index_with_base(source);

    let diagnostics = index.diagnostics();
    assert_eq!(diagnostics.len(), 1);
    match &diagnostics[0] {
        SemanticDiagnostic::LazyShadowAmbiguity {
            name,
            call_range,
            overwrite_range,
        } => {
            assert_eq!(name, "local");

            let call_start = u32::from(call_range.start()) as usize;
            let call_end = u32::from(call_range.end()) as usize;
            assert_eq!(&source[call_start..call_end], "local({ x <- 1 })");

            let overwrite_start = u32::from(overwrite_range.start()) as usize;
            let overwrite_end = u32::from(overwrite_range.end()) as usize;
            assert_eq!(&source[overwrite_start..overwrite_end], "local");
        },
    }
}

#[test]
fn test_diagnostic_none_when_callee_unbound_everywhere() {
    // `local` is never bound anywhere, so the NSE decision is certain and
    // nothing competes with it. No diagnostic.
    let index = index_with_base(
        "\
x <- 1
f <- function() local({ x })
",
    );
    assert!(index.diagnostics().is_empty());
}

#[test]
fn test_diagnostic_none_with_eager_predecessor() {
    // `local` is bound before `f` is defined, a sure shadow, so `f`'s `local()`
    // is not NSE at all. No diagnostic.
    let index = index_with_base(
        "\
local <- identity
f <- function() local({ x })
",
    );
    assert!(index.diagnostics().is_empty());
}

// --- Eager linear scan: descent and pending names ---

#[test]
fn test_nse_descent_consults_each_call_once() {
    // The inner `local` sits inside the outer `local`'s eager body. The descent
    // scans it once and the walk installs the pending names instead of
    // re-scanning, so each of the two calls reaches the resolver exactly once.
    let resolver = TestImportsResolver::with_base();
    let consultations = resolver.consultations();

    build_with("local({ local({ x <- 1 }) })", resolver);

    assert_eq!(consultations.get(), 2);
}

#[test]
fn test_nse_descent_snapshot_through_pending_scope() {
    // The descent records `y` as pending for `local`'s scope; the walk installs
    // it before walking `f`, so `f`'s use of `y` resolves to the enclosing
    // snapshot in `local`.
    let index = index_with_base(
        "\
local({
    y <- 1
    f <- function() y
})
",
    );
    let file = ScopeId::from(0);
    let local_scope = ScopeId::from(1);
    let f_scope = ScopeId::from(2);

    assert_eq!(
        index.scope(local_scope).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
    );
    assert_eq!(index.scope(local_scope).parent(), Some(file));
    assert_eq!(index.scope(f_scope).kind(), ScopeKind::Function);
    assert_eq!(index.scope(f_scope).parent(), Some(local_scope));

    // `y` lands in local's scope.
    assert_eq!(
        index.symbols(local_scope).get("y").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );

    // `f`'s use of `y` resolves to local's snapshot. In local, `y` is
    // DefinitionId 0 (`f` is DefinitionId 1).
    let (enclosing_scope, bindings) = index.enclosing_bindings(f_scope, UseId::from(0)).unwrap();
    assert_eq!(enclosing_scope, local_scope);
    assert_eq!(bindings.definitions(), &[DefinitionId::from(0)]);
}

#[test]
fn test_nse_descent_eager_under_lazy() {
    // `local` resolves during `f`'s walk-time scan (unit = `f`), which descends
    // into the body and records its names as pending. `x` lands in local's
    // Nested+Eager scope, not in `f`.
    let index = index_with_base(
        "\
f <- function() {
    local({
        x <- 1
    })
}
",
    );
    let f_scope = ScopeId::from(1);
    let local_scope = ScopeId::from(2);

    assert_eq!(index.scope(f_scope).kind(), ScopeKind::Function);
    assert_eq!(
        index.scope(local_scope).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
    );
    assert_eq!(index.scope(local_scope).parent(), Some(f_scope));

    assert!(index.symbols(f_scope).get("x").is_none());
    assert_eq!(
        index.symbols(local_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_descent_nested_eager_in_eager() {
    // `local({ local({ y <- 1 }) })`: descent stack depth 2, each body's names
    // pending under its own range. `y` lands in the inner scope.
    let index = index_with_base(
        "\
local({
    local({
        y <- 1
    })
})
",
    );
    let file = ScopeId::from(0);
    let outer_local = ScopeId::from(1);
    let inner_local = ScopeId::from(2);

    assert_eq!(index.scope_ids().count(), 3);
    assert_eq!(
        index.scope(outer_local).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
    );
    assert_eq!(index.scope(outer_local).parent(), Some(file));
    assert_eq!(
        index.scope(inner_local).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
    );
    assert_eq!(index.scope(inner_local).parent(), Some(outer_local));

    assert!(index.symbols(outer_local).get("y").is_none());
    assert_eq!(
        index.symbols(inner_local).get("y").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_descent_lazy_flag_eager_vs_lazy_context() {
    // An eager callee at file scope consults with `lazy = false`; the same
    // callee inside a function body consults with `lazy = true`.
    let resolver = TestImportsResolver::with_base();
    let log = resolver.consultation_log();

    build_with(
        "\
local({ x <- 1 })
f <- function() {
    local({ y <- 1 })
}
",
        resolver,
    );

    let records = log.borrow();
    let local_lazy: Vec<bool> = records
        .iter()
        .filter(|(name, _lazy)| name == "local")
        .map(|(_name, lazy)| *lazy)
        .collect();
    assert_eq!(local_lazy, vec![false, true]);
}

#[test]
fn test_nse_descent_eager_in_eager_in_function_stays_lazy() {
    // An eager `local` nested inside another eager `local` inside a function
    // still consults with `lazy = true`. Laziness is a property of the enclosing
    // scan unit (the function), which the descent preserves by keeping
    // `current_scope` on the function while it scans both eager bodies inline. If
    // the inner `local` were resolved against its immediate eager scope instead,
    // `is_lazy()` would read `false` and the flag would regress.
    let resolver = TestImportsResolver::with_base();
    let log = resolver.consultation_log();

    build_with(
        "\
f <- function() {
    local({
        local({ x <- 1 })
    })
}
",
        resolver,
    );

    let records = log.borrow();
    let local_lazy: Vec<bool> = records
        .iter()
        .filter(|(name, _lazy)| name == "local")
        .map(|(_name, lazy)| *lazy)
        .collect();
    assert_eq!(local_lazy, vec![true, true]);
}

// --- Attach tracking ---

#[test]
fn test_nse_attach_eager_body_inside_lazy_body_is_deferred() {
    // `local` is eager, but it sits inside `f`'s function body, so reaching
    // its `library(shiny)` waits on `f()` being called. The attach is recorded
    // (at `local`'s eager scope) but does not run at the file's top level: the
    // eager `attached_packages()` must exclude it, only `_anywhere()` sees it.
    // Guards against a one-scope `is_lazy()` check that misses the lazy
    // ancestor.
    let index = index_with_base(
        "\
f <- function() {
    local({
        library(shiny)
    })
}
",
    );
    let f_scope = ScopeId::from(1);
    let local_scope = ScopeId::from(2);

    assert!(index.attached_packages().is_empty());
    assert_eq!(index.attached_packages_anywhere(), vec!["shiny"]);
    assert_eq!(index.scope(f_scope).kind(), ScopeKind::Function);
    assert_eq!(
        index.scope(local_scope).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager)
    );
}

#[test]
fn test_nse_attach_recognition_respects_shadowing() {
    // `library` is rebound before the attach call, so `library(shiny)` isn't an
    // attach: shiny never attaches and the later `reactive` is not NSE. The bug
    // this fixes: a syntactic `fn_name == "library"` match recorded a bogus
    // attach here.
    let index = index_with_base(
        "\
library <- quote
library(shiny)
reactive({
    y <- 1
})
",
    );
    let file = ScopeId::from(0);

    assert!(index.attached_packages().is_empty());
    assert_eq!(index.scope_ids().count(), 1);
    assert_eq!(
        index.symbols(file).get("y").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_attach_shadow_confined_to_nse_scope() {
    // The `library` rebind is confined to `local`'s scope, so the top-level
    // `library(shiny)` sees the unshadowed `library` and attaches. The descent
    // keeps the rebind from leaking, so `reactive` is NSE.
    let index = index_with_base(
        "\
local({
    library <- quote
})
library(shiny)
reactive({
    y <- 1
})
",
    );
    let reactive_scope = ScopeId::from(2);

    assert_eq!(index.attached_packages(), vec!["shiny"]);
    assert_eq!(
        index.scope(reactive_scope).kind(),
        ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Lazy)
    );
}

#[test]
fn test_nse_attach_in_body_verb_shadow_determined() {
    // Inside `local`, `library` is rebound before `library(shiny)`, so the
    // descent resolves the rebind first and the attach call is not an attach.
    // shiny never attaches and `reactive` is not NSE. Neither the rebind nor a
    // (non-)attach leaks out of `local`.
    let index = index_with_base(
        "\
local({
    library <- quote
    library(shiny)
})
reactive({
    y <- 1
})
",
    );
    let file = ScopeId::from(0);

    assert!(index.attached_packages().is_empty());
    assert_eq!(index.scope_ids().count(), 2);
    assert_eq!(
        index.symbols(file).get("y").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_nse_attach_within_lazy_body_not_yet_supported() {
    // Sequential-within-one-lazy-body: when `f` runs, `library(shiny)` runs
    // before `reactive`, so `reactive` is determinately NSE. We don't promote it
    // today: `attached_flow` only grows in eager context, so the attach inside
    // `f` (a lazy body) isn't visible to `reactive` in the same body. The attach
    // is still recorded as a `SemanticCall::Attach`. This could be supported by
    // tracking a per-unit attach set seeded from the EOF view, parallel to
    // `bound_so_far`; deferred for now.
    let index = index_with_base(
        "\
f <- function() {
    library(shiny)
    reactive({
        x <- 1
    })
}
",
    );
    let f_scope = ScopeId::from(1);

    // The attach is recorded (scoped to `f`), but not fed to `reactive`, and
    // not counted at the file's top level: only `attached_packages_anywhere()`
    // sees a `library()` buried in a function body.
    assert_eq!(index.attached_packages_anywhere(), vec!["shiny"]);
    assert!(index.attached_packages().is_empty());
    assert_eq!(index.scope_ids().count(), 2);
    assert_eq!(index.scope(f_scope).kind(), ScopeKind::Function);
}
