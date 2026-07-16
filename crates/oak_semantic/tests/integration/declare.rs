//! Local `declare()`: a function annotating its own calling convention, so
//! calls to it resolve to the declared effects.

use aether_parser::parse;
use aether_parser::RParserOptions;
use oak_semantic::effects::DeclareDiagnosticKind;
use oak_semantic::semantic_index::NseScope;
use oak_semantic::semantic_index::NseTiming;
use oak_semantic::semantic_index::ScopeId;
use oak_semantic::semantic_index::ScopeKind;
use oak_semantic::semantic_index::SemanticCallKind;
use oak_semantic::semantic_index::SemanticDiagnostic;
use oak_semantic::semantic_index::SemanticIndex;
use oak_semantic::semantic_index::SymbolFlags;

use crate::resolvers::TestImportsResolver;

fn index(source: &str) -> SemanticIndex {
    let parsed = parse(source, RParserOptions::default());
    if parsed.has_error() {
        panic!("source has syntax errors: {source}");
    }
    oak_semantic::build_index(&parsed.tree(), TestImportsResolver::with_base())
}

/// Whether `name` is recorded as a use anywhere in the index.
fn is_used(index: &SemanticIndex, name: &str) -> bool {
    !index.uses_of(name).is_empty()
}

// --- Quote ---

#[test]
fn test_local_quote_suppresses_argument() {
    // The call's argument is captured, so `foo` is not a use. The directive
    // itself contributes nothing: `x` and `Quote` are not uses either.
    let index = index(
        "\
my_quote <- function(x) {
    declare(x = Quote)
}
my_quote(foo)
",
    );

    assert!(!is_used(&index, "foo"));
    assert!(!is_used(&index, "Quote"));
    assert!(!is_used(&index, "x"));
    assert!(index.diagnostics().is_empty());
}

#[test]
fn test_local_quote_unbraced_body() {
    // The directive is the whole (unbraced) body, and is skipped just the same.
    let index = index(
        "\
my_quote <- function(x) declare(x = Quote)
my_quote(foo)
",
    );

    assert!(!is_used(&index, "foo"));
    assert!(index.diagnostics().is_empty());
}

// --- Nse, each scope/timing combo ---

#[test]
fn test_local_nse_nested_eager_pushes_scope() {
    // `local`-like: a Nested + Eager scope, with the body's bindings landing in
    // it (visible where the call sits, like `local()`).
    let index = index(
        "\
my_local <- function(x) declare(x = Nse(\"nested\"))
my_local({
    y <- 1
})
",
    );
    let file = ScopeId::from(0);
    let nse_scope = ScopeId::from(2);

    assert_eq!(
        index.scope(nse_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
    assert!(index.symbols(file).get("y").is_none());
    assert_eq!(
        index.symbols(nse_scope).get("y").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_local_nse_current_eager_no_scope() {
    // `evalq`-like: Current + Eager pushes no scope, so `y` binds in the current
    // (file) scope.
    let index = index(
        "\
my_evalq <- function(x) declare(x = Nse(\"current\"))
my_evalq({
    y <- 1
})
",
    );
    let file = ScopeId::from(0);

    // File and `my_evalq`'s function scope only, no NSE scope.
    assert_eq!(index.scope_ids().count(), 2);
    assert_eq!(
        index.symbols(file).get("y").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_local_nse_nested_lazy_own_scope() {
    // `reactive`-like: Nested + Lazy gets its own deferred scope.
    let index = index(
        "\
my_reactive <- function(x) declare(x = Nse(\"nested\", eager = FALSE))
my_reactive({
    y <- 1
})
",
    );
    let file = ScopeId::from(0);
    let nse_scope = ScopeId::from(2);

    assert_eq!(
        index.scope(nse_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Lazy)
    );
    assert!(index.symbols(file).get("y").is_none());
    assert_eq!(
        index.symbols(nse_scope).get("y").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_local_nse_current_lazy_routes_to_owner() {
    // `on_load`-like: Current + Lazy pushes a scope (for lazy resolution) but
    // routes its definitions to the owner (file) scope.
    let index = index(
        "\
my_on_load <- function(x) declare(x = Nse(\"current\", eager = FALSE))
my_on_load({
    y <- 1
})
",
    );
    let file = ScopeId::from(0);
    let nse_scope = ScopeId::from(2);

    assert_eq!(
        index.scope(nse_scope).kind(),
        ScopeKind::Nse(NseScope::Current, NseTiming::Lazy)
    );
    assert_eq!(
        index.symbols(file).get("y").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
    assert!(index.symbols(nse_scope).get("y").is_none());
}

// --- Local shadows the registry ---

#[test]
fn test_local_declaration_shadows_base() {
    // A local `quote` with a Current + Eager declaration overrides base
    // `quote`'s Quote behavior, so `x <- 1` evaluates and binds in the file
    // scope rather than being captured.
    let index = index(
        "\
quote <- function(x) declare(x = Nse(\"current\"))
quote(x <- 1)
",
    );
    let file = ScopeId::from(0);

    assert_eq!(index.scope_ids().count(), 2);
    assert_eq!(
        index.symbols(file).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

// --- Rebind drops the declaration ---

#[test]
fn test_rebind_drops_declaration() {
    // Last-write-wins in the flow state: the call before the rebind resolves via
    // the declaration (so `a` is captured), the call after it sees a plain
    // function (so `b` is an ordinary use).
    let index = index(
        "\
my_fn <- function(x) declare(x = Quote)
my_fn(a)
my_fn <- function(x) x
my_fn(b)
",
    );

    assert!(!is_used(&index, "a"));
    assert!(is_used(&index, "b"));
    // Both calls are at eager file level, so no lazy-crossing ambiguity.
    assert!(index.diagnostics().is_empty());
}

// --- Lazy forward reference ---

#[test]
fn test_lazy_forward_reference_resolves() {
    // `f`'s body runs later, so it can reference `my_quote` defined below it.
    // The declaration is unanimous (one binding), so this resolves with no lint.
    let index = index(
        "\
f <- function() my_quote(foo)
my_quote <- function(x) declare(x = Quote)
",
    );

    assert!(!is_used(&index, "foo"));
    assert!(index.diagnostics().is_empty());
}

// --- Mixed-binding ambiguity ---

#[test]
fn test_mixed_binding_lints_but_resolves_flow_precise() {
    // `f` inherits the declaring `my_fn` (so `foo` is captured), but the binding
    // scope later rebinds `my_fn` plain. Whole-scope the bindings disagree
    // (`Mixed`), and `f`'s timing relative to the rebind is unknowable, so the
    // ambiguity is flagged. Resolution still uses the flow-precise declaration.
    let index = index(
        "\
my_fn <- function(x) declare(x = Quote)
f <- function() my_fn(foo)
my_fn <- function(x) x
",
    );

    assert!(!is_used(&index, "foo"));

    let diagnostics = index.diagnostics();
    assert_eq!(diagnostics.len(), 1);
    match &diagnostics[0] {
        SemanticDiagnostic::DeclaredMixedAmbiguity { name, .. } => assert_eq!(name, "my_fn"),
        other => panic!("unexpected diagnostic: {other:?}"),
    }
}

#[test]
fn test_same_scan_unit_rebind_no_lint() {
    // Declare-bind, call, then plain rebind all inside one function body. The
    // call is flow-precise (no lazy crossing), so it resolves via the
    // declaration with no ambiguity lint.
    let index = index(
        "\
outer <- function() {
    my_fn <- function(x) declare(x = Quote)
    my_fn(foo)
    my_fn <- function(x) x
}
",
    );

    assert!(!is_used(&index, "foo"));
    assert!(index.diagnostics().is_empty());
}

// --- Misplaced and malformed directives ---

#[test]
fn test_misplaced_declare_is_flagged_and_inert() {
    // A `declare()` that isn't the body's first statement is misplaced: its
    // arguments are suppressed (no `Quote` use), it gives the function no
    // declaration (so `bar` is an ordinary use), and the spot is flagged.
    let index = index(
        "\
f <- function(x) {
    y <- 1
    declare(x = Quote)
}
f(bar)
",
    );

    assert!(!is_used(&index, "Quote"));
    assert!(is_used(&index, "bar"));

    let misplaced = index
        .diagnostics()
        .iter()
        .filter(|d| matches!(d, SemanticDiagnostic::MisplacedDeclare { .. }))
        .count();
    assert_eq!(misplaced, 1);
}

#[test]
fn test_parse_diagnostic_surfaces() {
    // A directive naming a formal the signature doesn't have surfaces the
    // parser's `UnknownFormal` diagnostic through the index.
    let index = index("f <- function(x) declare(z = Quote)");

    let diagnostics = index.diagnostics();
    assert_eq!(diagnostics.len(), 1);
    match &diagnostics[0] {
        SemanticDiagnostic::MalformedDeclaration(diagnostic) => assert!(matches!(
            &diagnostic.kind,
            DeclareDiagnosticKind::UnknownFormal { name } if name == "z"
        )),
        other => panic!("unexpected diagnostic: {other:?}"),
    }
}

// --- Local environment effects ---

#[test]
fn test_local_attach_effect() {
    // A local `Attach` declaration reading its package via `substitute` records
    // an attach for `dplyr` and leaves `dplyr` inert (not a use).
    let index = index(
        "\
my_lib <- function(pkg) declare(Attach(.(substitute(pkg))))
my_lib(dplyr)
",
    );

    assert_eq!(index.attached_packages(), vec!["dplyr"]);
    assert!(!is_used(&index, "dplyr"));
}

#[test]
fn test_local_source_effect() {
    // A local `Source` declaration records a source semantic call for the given
    // path, the same as the registry `source` does. `envir = .(parent.frame())`
    // targets the call site, which at file scope is where the names land.
    let index = index(
        "\
my_source <- function(file) declare(Source(.(file), envir = .(parent.frame())))
my_source(\"helpers.R\")
",
    );

    let sources: Vec<&SemanticCallKind> = index
        .semantic_calls()
        .iter()
        .map(|call| call.kind())
        .collect();
    assert_eq!(sources, [&SemanticCallKind::Source {
        path: "helpers.R".into(),
        resolved: None,
    }]);
}

// --- Eager forward reference (registry path, no declaration yet) ---

#[test]
fn test_eager_forward_reference_is_a_use() {
    // At the eager call, `my_quote` is not yet bound and no declaration exists,
    // so it takes the registry path (which doesn't annotate it) and `foo` stays
    // an ordinary use.
    let index = index(
        "\
my_quote(foo)
my_quote <- function(x) declare(x = Quote)
",
    );

    assert!(is_used(&index, "foo"));
    assert!(index.diagnostics().is_empty());
}
