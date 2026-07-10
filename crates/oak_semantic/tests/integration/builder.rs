use aether_parser::parse;
use aether_parser::RParserOptions;
use aether_syntax::RCall;
use aether_syntax::RSyntaxKind;
use oak_semantic::build_index;
use oak_semantic::effects::CallContext;
use oak_semantic::effects::EffectHandler;
use oak_semantic::effects::SourceAnnotation;
use oak_semantic::effects_registry;
use oak_semantic::semantic_index::DefinitionId;
use oak_semantic::semantic_index::DefinitionKind;
use oak_semantic::semantic_index::NamespaceAccessKind;
use oak_semantic::semantic_index::ScopeId;
use oak_semantic::semantic_index::ScopeKind;
use oak_semantic::semantic_index::SemanticCallKind;
use oak_semantic::semantic_index::SemanticIndex;
use oak_semantic::semantic_index::SymbolFlags;
use oak_semantic::semantic_index::UseId;
use oak_semantic::EffectsHandlers;
use oak_semantic::ImportsResolver;
use oak_semantic::NoopImportsResolver;
use oak_semantic::SourceResolution;
use url::Url;

use crate::resolvers::TestImportsResolver;

fn index(source: &str) -> SemanticIndex {
    let parsed = parse(source, RParserOptions::default());

    if parsed.has_error() {
        panic!("source has syntax errors: {source}");
    }

    build_index(&parsed.tree(), NoopImportsResolver)
}

/// Build with base attached. Attach recognition (`library()`/`require()`) runs
/// on the resolve path now, so it needs a resolver that resolves base, unlike
/// the resolver-independent `source()` recognition the `index()` helper covers.
fn index_with_base(source: &str) -> SemanticIndex {
    let parsed = parse(source, RParserOptions::default());

    if parsed.has_error() {
        panic!("source has syntax errors: {source}");
    }

    build_index(&parsed.tree(), TestImportsResolver::with_base())
}

fn semantic_call_kinds(index: &SemanticIndex) -> Vec<&SemanticCallKind> {
    index.semantic_calls().iter().map(|c| c.kind()).collect()
}

#[test]
fn test_empty_file() {
    let index = index("");
    let file = ScopeId::from(0);
    assert_eq!(index.scope(file).kind(), ScopeKind::File);
    assert_eq!(index.symbols(file).len(), 0);
}

#[test]
fn test_simple_assignment() {
    let index = index("x <- 1");
    let file = ScopeId::from(0);

    assert_eq!(index.symbols(file).len(), 1);

    let sym = index.symbols(file).get("x").unwrap();
    assert_eq!(sym.flags(), SymbolFlags::IS_BOUND);

    assert_eq!(index.definitions(file).len(), 1);
    let DefinitionKind::Assignment(node) = index.definitions(file)[DefinitionId::from(0)].kind()
    else {
        panic!("expected Assignment");
    };
    assert_eq!(
        node.syntax_node_ptr().kind(),
        RSyntaxKind::R_BINARY_EXPRESSION
    );
    assert_eq!(index.uses(file).len(), 0);
}

#[test]
fn test_equals_assignment() {
    let index = index("x = 1");
    let file = ScopeId::from(0);

    let sym = index.symbols(file).get("x").unwrap();
    assert_eq!(sym.flags(), SymbolFlags::IS_BOUND);
}

#[test]
fn test_assignment_with_use() {
    let index = index("x <- y");
    let file = ScopeId::from(0);

    assert_eq!(index.symbols(file).len(), 2);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_BOUND);

    let y = index.symbols(file).get("y").unwrap();
    assert_eq!(y.flags(), SymbolFlags::IS_USED);

    assert_eq!(index.definitions(file).len(), 1);
    assert_eq!(index.uses(file).len(), 1);
}

#[test]
fn test_rhs_collected_before_lhs() {
    // The first use site should be `y` (RHS) and the first definition site should be `x` (LHS).
    let index = index("x <- y");
    let file = ScopeId::from(0);

    let use_site = &index.uses(file)[UseId::from(0)];
    let use_sym = index.symbols(file).symbol(use_site.symbol());
    assert_eq!(use_sym.name(), "y");

    let def_site = &index.definitions(file)[DefinitionId::from(0)];
    let def_sym = index.symbols(file).symbol(def_site.symbol());
    assert_eq!(def_sym.name(), "x");
}

#[test]
fn test_multiple_assignments_same_symbol() {
    let index = index("x <- 1\nx <- 2");
    let file = ScopeId::from(0);

    // One symbol, two definition sites
    assert_eq!(index.symbols(file).len(), 1);
    assert_eq!(index.definitions(file).len(), 2);
}

#[test]
fn test_function_creates_scope() {
    let index = index("f <- function(x) x");
    let file = ScopeId::from(0);
    let fun_scope = ScopeId::from(1);

    // File scope has `f`
    assert_eq!(index.symbols(file).len(), 1);
    assert_eq!(
        index.symbols(file).get("f").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );

    // Function scope
    assert_eq!(index.scope(fun_scope).kind(), ScopeKind::Function);
    assert_eq!(index.scope(fun_scope).parent(), Some(file));

    // Function scope has `x` as parameter and use
    let x = index.symbols(fun_scope).get("x").unwrap();
    assert_eq!(
        x.flags(),
        SymbolFlags::IS_BOUND
            .union(SymbolFlags::IS_PARAMETER)
            .union(SymbolFlags::IS_USED)
    );

    assert_eq!(index.definitions(fun_scope).len(), 1);
    let DefinitionKind::Parameter(node) =
        index.definitions(fun_scope)[DefinitionId::from(0)].kind()
    else {
        panic!("expected Parameter");
    };
    assert_eq!(node.syntax_node_ptr().kind(), RSyntaxKind::R_PARAMETER);
    assert_eq!(index.uses(fun_scope).len(), 1);
}

#[test]
fn test_nested_functions() {
    let index = index("f <- function(x) function(y) x + y");
    let file = ScopeId::from(0);
    let outer = ScopeId::from(1);
    let inner = ScopeId::from(2);

    // File scope: `f`
    assert_eq!(index.symbols(file).len(), 1);

    // Outer function: `x` as parameter (no use in this scope)
    assert_eq!(index.scope(outer).kind(), ScopeKind::Function);
    let x = index.symbols(outer).get("x").unwrap();
    assert_eq!(
        x.flags(),
        SymbolFlags::IS_BOUND.union(SymbolFlags::IS_PARAMETER)
    );

    // Inner function: `y` as parameter+use, `x` as use
    assert_eq!(index.scope(inner).kind(), ScopeKind::Function);
    assert_eq!(index.scope(inner).parent(), Some(outer));

    let y = index.symbols(inner).get("y").unwrap();
    assert_eq!(
        y.flags(),
        SymbolFlags::IS_BOUND
            .union(SymbolFlags::IS_PARAMETER)
            .union(SymbolFlags::IS_USED)
    );

    let x_inner = index.symbols(inner).get("x").unwrap();
    assert_eq!(x_inner.flags(), SymbolFlags::IS_USED);
}

#[test]
fn test_parameter_default_uses() {
    let index = index("function(x = y) x");
    let fun_scope = ScopeId::from(1);

    // `y` in the default is a use in the function scope
    assert!(index.symbols(fun_scope).get("y").is_some());
    let y = index.symbols(fun_scope).get("y").unwrap();
    assert_eq!(y.flags(), SymbolFlags::IS_USED);
}

#[test]
fn test_call_argument_equals_not_assignment() {
    // `x = 1` inside a call argument is NOT an assignment
    let index = index("f(x = 1)");
    let file = ScopeId::from(0);

    // Only `f` is recorded (as a use), `x` is an argument name, not a definition
    assert_eq!(index.symbols(file).len(), 1);
    assert!(index.symbols(file).get("f").is_some());
    assert!(index.symbols(file).get("x").is_none());
}

#[test]
fn test_complex_lhs_not_binding() {
    // `x$foo <- 1` -- `x` is a use, not a binding
    let index = index("x$foo <- 1");
    let file = ScopeId::from(0);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_USED);

    assert!(index.symbols(file).get("foo").is_none());
    assert_eq!(index.definitions(file).len(), 0);
}

#[test]
fn test_resolve_symbol_in_scope() {
    let index = index("x <- 1\nf <- function(y) x + y");
    let inner = ScopeId::from(1);

    // `y` resolves in the function scope
    let (scope, _, _) = index.resolve("y", inner).unwrap();
    assert_eq!(scope, inner);

    // `x` resolves in the file scope
    let file = ScopeId::from(0);
    let (scope, _, _) = index.resolve("x", inner).unwrap();
    assert_eq!(scope, file);

    // `z` doesn't resolve
    assert!(index.resolve("z", inner).is_none());
}

#[test]
fn test_resolve_prefers_inner_scope() {
    let index = index("x <- 1\nf <- function(x) x");
    let file = ScopeId::from(0);
    let fun = ScopeId::from(1);

    // From function scope, `x` resolves to the function's own `x`
    let (scope, _, _) = index.resolve("x", fun).unwrap();
    assert_eq!(scope, fun);

    // From file scope, `x` resolves to file's `x`
    let (scope, _, _) = index.resolve("x", file).unwrap();
    assert_eq!(scope, file);
}

#[test]
fn test_scope_at() {
    let source = "x <- 1\nf <- function(y) y";
    let idx = index(source);
    let file = ScopeId::from(0);
    let fun = ScopeId::from(1);

    // Offset 0 is in `x` -- file scope
    assert_eq!(idx.scope_at(biome_rowan::TextSize::from(0)).0, file);

    // Offset inside the function body
    let body_offset = source.find(") y").unwrap() + 2;
    assert_eq!(
        idx.scope_at(biome_rowan::TextSize::from(body_offset as u32))
            .0,
        fun
    );
}

#[test]
fn test_child_scopes() {
    let index = index("f <- function(x) x\ng <- function(y) y");
    let file = ScopeId::from(0);

    let children: Vec<_> = index.child_scope_ids(file).collect();
    assert_eq!(children.len(), 2);
}

#[test]
fn test_ancestor_scopes() {
    let index = index("f <- function(x) function(y) y");
    let inner = ScopeId::from(2);
    let outer = ScopeId::from(1);
    let file = ScopeId::from(0);

    let ancestors: Vec<_> = index.ancestor_scope_ids(inner).collect();
    assert_eq!(ancestors, vec![inner, outer, file]);
}

#[test]
fn test_for_loop_body() {
    let idx = index("for (i in xs) print(i)");
    let file = ScopeId::from(0);

    let xs = idx.symbols(file).get("xs").unwrap();
    assert_eq!(xs.flags(), SymbolFlags::IS_USED);

    let print = idx.symbols(file).get("print").unwrap();
    assert_eq!(print.flags(), SymbolFlags::IS_USED);

    let i = idx.symbols(file).get("i").unwrap();
    assert_eq!(i.flags(), SymbolFlags::IS_BOUND.union(SymbolFlags::IS_USED));

    let DefinitionKind::ForVariable(node) = idx.definitions(file)[DefinitionId::from(0)].kind()
    else {
        panic!("expected ForVariable");
    };
    assert_eq!(node.syntax_node_ptr().kind(), RSyntaxKind::R_FOR_STATEMENT);
}

#[test]
fn test_if_else() {
    let index = index("if (cond) a else b");
    let file = ScopeId::from(0);

    let cond = index.symbols(file).get("cond").unwrap();
    assert_eq!(cond.flags(), SymbolFlags::IS_USED);

    let a = index.symbols(file).get("a").unwrap();
    assert_eq!(a.flags(), SymbolFlags::IS_USED);

    let b = index.symbols(file).get("b").unwrap();
    assert_eq!(b.flags(), SymbolFlags::IS_USED);
}

#[test]
fn test_braced_expression_assignments() {
    // Assignments inside `{}` are in the same scope (no new scope for braces)
    let index = index("f <- function() {\n  x <- 1\n  y <- x\n}");
    let fun = ScopeId::from(1);

    assert_eq!(index.definitions(fun).len(), 2);
    assert!(index.symbols(fun).get("x").is_some());
    assert!(index.symbols(fun).get("y").is_some());

    let x = index.symbols(fun).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_BOUND.union(SymbolFlags::IS_USED));
}

#[test]
fn test_dots_parameter() {
    let index = index("function(...) list(...)");
    let fun = ScopeId::from(1);

    let dots = index.symbols(fun).get("...").unwrap();
    assert_eq!(
        dots.flags(),
        SymbolFlags::IS_BOUND
            .union(SymbolFlags::IS_PARAMETER)
            .union(SymbolFlags::IS_USED)
    );
}

#[test]
fn test_dot_dot_i_parameter() {
    let index = index("function(..1, ..2) list(..1, ..2)");
    let fun = ScopeId::from(1);

    let dot1 = index.symbols(fun).get("..1").unwrap();
    assert_eq!(
        dot1.flags(),
        SymbolFlags::IS_BOUND
            .union(SymbolFlags::IS_PARAMETER)
            .union(SymbolFlags::IS_USED)
    );

    let dot2 = index.symbols(fun).get("..2").unwrap();
    assert_eq!(
        dot2.flags(),
        SymbolFlags::IS_BOUND
            .union(SymbolFlags::IS_PARAMETER)
            .union(SymbolFlags::IS_USED)
    );
}

#[test]
fn test_arrow_assignment_in_if_condition() {
    // `<-` is always an assignment, even inside `if()`
    let index = index("if (x <- f()) x");
    let file = ScopeId::from(0);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_BOUND.union(SymbolFlags::IS_USED));
    assert_eq!(index.definitions(file).len(), 1);
}

#[test]
fn test_arrow_assignment_in_call_argument() {
    // `<-` in a call argument still creates a definition in the enclosing scope
    let index = index("f(x <- 1)");
    let file = ScopeId::from(0);

    assert!(index.symbols(file).get("x").is_some());
    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_BOUND);
    assert_eq!(index.definitions(file).len(), 1);
}

#[test]
fn test_equals_in_call_argument_still_not_assignment() {
    // `=` in a call argument is NOT an assignment (unchanged)
    let index = index("f(x = 1)");
    let file = ScopeId::from(0);

    assert!(index.symbols(file).get("x").is_none());
    assert_eq!(index.definitions(file).len(), 0);
}

#[test]
fn test_parenthesized_arrow_assignment() {
    // `<-` is always an assignment, even inside parentheses
    let index = index("(x <- 1)");
    let file = ScopeId::from(0);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_BOUND);
    assert_eq!(index.definitions(file).len(), 1);
}

#[test]
fn test_parenthesized_equals_is_assignment() {
    // `(x = 1)` -- `=` as a `RBinaryExpression` is always an assignment.
    // In call arguments, `=` is consumed by the parser into
    // `RArgumentNameClause` and never appears as a binary expression.
    let index = index("(x = 1)");
    let file = ScopeId::from(0);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_BOUND);
    assert_eq!(index.definitions(file).len(), 1);
}

#[test]
fn test_right_assignment() {
    let index = index("1 -> x");
    let file = ScopeId::from(0);

    assert_eq!(index.symbols(file).len(), 1);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_BOUND);

    assert_eq!(index.definitions(file).len(), 1);
    assert_eq!(index.uses(file).len(), 0);
}

#[test]
fn test_right_assignment_with_use() {
    let index = index("y -> x");
    let file = ScopeId::from(0);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_BOUND);

    let y = index.symbols(file).get("y").unwrap();
    assert_eq!(y.flags(), SymbolFlags::IS_USED);
}

#[test]
fn test_right_assignment_rhs_collected_before_lhs() {
    let index = index("y -> x");
    let file = ScopeId::from(0);

    let use_site = &index.uses(file)[UseId::from(0)];
    let use_sym = index.symbols(file).symbol(use_site.symbol());
    assert_eq!(use_sym.name(), "y");

    let def_site = &index.definitions(file)[DefinitionId::from(0)];
    let def_sym = index.symbols(file).symbol(def_site.symbol());
    assert_eq!(def_sym.name(), "x");
}

#[test]
fn test_right_assignment_complex_target() {
    // `1 -> x$foo` -- `x` is a use, not a binding
    let index = index("1 -> x$foo");
    let file = ScopeId::from(0);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_USED);

    assert_eq!(index.definitions(file).len(), 0);
}

#[test]
fn test_subset_assignment_complex_lhs() {
    let index = index("x[1] <- value");
    let file = ScopeId::from(0);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_USED);

    let value = index.symbols(file).get("value").unwrap();
    assert_eq!(value.flags(), SymbolFlags::IS_USED);

    assert_eq!(index.definitions(file).len(), 0);
}

#[test]
fn test_double_bracket_assignment_complex_lhs() {
    let index = index("x[[1]] <- value");
    let file = ScopeId::from(0);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_USED);

    assert_eq!(index.definitions(file).len(), 0);
}

#[test]
fn test_at_extraction() {
    let index = index("x@slot");
    let file = ScopeId::from(0);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_USED);

    // `slot` is not recorded as a use
    assert!(index.symbols(file).get("slot").is_none());
    assert_eq!(index.symbols(file).len(), 1);
}

#[test]
fn test_namespace_expression_no_uses() {
    // Both sides of `::` are selectors, not variable references in the current
    // scope, so neither the package nor the symbol becomes a use or a symbol.
    let index = index("dplyr::filter");
    let file = ScopeId::from(0);

    assert!(index.symbols(file).get("dplyr").is_none());
    assert!(index.symbols(file).get("filter").is_none());
    assert_eq!(index.symbols(file).len(), 0);
    assert_eq!(index.uses(file).len(), 0);
}

#[test]
fn test_triple_colon_namespace_no_uses() {
    let index = index("pkg:::internal_fn");
    let file = ScopeId::from(0);

    assert_eq!(index.symbols(file).len(), 0);
}

#[test]
fn test_while_loop() {
    let index = index("while (cond) x");
    let file = ScopeId::from(0);

    let cond = index.symbols(file).get("cond").unwrap();
    assert_eq!(cond.flags(), SymbolFlags::IS_USED);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_USED);
}

#[test]
fn test_repeat_loop() {
    let index = index("repeat x");
    let file = ScopeId::from(0);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_USED);
}

#[test]
fn test_super_assignment_at_file_scope() {
    // At file scope, `<<-` targets the file scope itself (no parent to walk
    // to), so the marker scope and the binding scope coincide. The symbol gets
    // both IS_SUPER_BOUND and IS_BOUND from a single recording.
    let index = index("x <<- 1");
    let file = ScopeId::from(0);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(
        x.flags(),
        SymbolFlags::IS_SUPER_BOUND.union(SymbolFlags::IS_BOUND)
    );

    // One definition, not a self-duplicate.
    assert_eq!(index.definitions(file).len(), 1);
    assert!(matches!(
        index.definitions(file)[DefinitionId::from(0)].kind(),
        DefinitionKind::SuperAssignment(_)
    ));
    assert_eq!(index.uses(file).len(), 0);
}

#[test]
fn test_super_assignment_right_at_file_scope() {
    let index = index("1 ->> x");
    let file = ScopeId::from(0);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(
        x.flags(),
        SymbolFlags::IS_SUPER_BOUND.union(SymbolFlags::IS_BOUND)
    );

    assert_eq!(index.definitions(file).len(), 1);
    assert!(matches!(
        index.definitions(file)[DefinitionId::from(0)].kind(),
        DefinitionKind::SuperAssignment(_)
    ));
    assert_eq!(index.uses(file).len(), 0);
}

#[test]
fn test_super_assignment_recorded_in_current_scope() {
    // `<<-` records the definition in the function scope where it lexically
    // appears AND an extra definition in the parent scope.
    let index = index("f <- function() { x <<- 1 }");
    let file = ScopeId::from(0);
    let fun = ScopeId::from(1);

    // File scope has `x` with IS_BOUND (extra definition from `<<-`)
    // and `f` with IS_BOUND. The `x <<-` definition is added during
    // function body processing, before `f <-`.
    let x_file = index.symbols(file).get("x").unwrap();
    assert_eq!(x_file.flags(), SymbolFlags::IS_BOUND);
    assert_eq!(index.definitions(file).len(), 2);
    assert!(matches!(
        index.definitions(file)[DefinitionId::from(0)].kind(),
        DefinitionKind::SuperAssignment(_)
    ));
    assert!(matches!(
        index.definitions(file)[DefinitionId::from(1)].kind(),
        DefinitionKind::Assignment(_)
    ));

    // Function scope has `x` with IS_SUPER_BOUND
    let x = index.symbols(fun).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_SUPER_BOUND);
    assert_eq!(index.definitions(fun).len(), 1);
    assert!(matches!(
        index.definitions(fun)[DefinitionId::from(0)].kind(),
        DefinitionKind::SuperAssignment(_)
    ));
}

#[test]
fn test_super_assignment_right_recorded_in_current_scope() {
    let index = index("f <- function() { 1 ->> x }");
    let file = ScopeId::from(0);
    let fun = ScopeId::from(1);

    let x_file = index.symbols(file).get("x").unwrap();
    assert_eq!(x_file.flags(), SymbolFlags::IS_BOUND);

    let x = index.symbols(fun).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_SUPER_BOUND);
}

#[test]
fn test_super_assignment_does_not_pollute_ancestor() {
    // `x <- 1` is in file scope, `x <<- 2` is in the function. The `<<-`
    // adds an extra definition to the file scope in addition to the
    // existing `x <- 1` assignment.
    let index = index("x <- 1\nf <- function() { x <<- 2 }");
    let file = ScopeId::from(0);
    let fun = ScopeId::from(1);

    // File scope: `x` has IS_BOUND (from both `<-` and `<<-`), `f` has IS_BOUND
    let x_file = index.symbols(file).get("x").unwrap();
    assert_eq!(x_file.flags(), SymbolFlags::IS_BOUND);

    let x_file_defs: Vec<_> = index
        .definitions(file)
        .iter()
        .filter(|(_, d)| index.symbols(file).symbol(d.symbol()).name() == "x")
        .collect();
    assert_eq!(x_file_defs.len(), 2);
    assert!(matches!(
        x_file_defs[0].1.kind(),
        DefinitionKind::Assignment(_)
    ));
    assert!(matches!(
        x_file_defs[1].1.kind(),
        DefinitionKind::SuperAssignment(_)
    ));

    // Function scope: `x` has IS_SUPER_BOUND from the `<<-`
    let x_fun = index.symbols(fun).get("x").unwrap();
    assert_eq!(x_fun.flags(), SymbolFlags::IS_SUPER_BOUND);
    assert_eq!(index.definitions(fun).len(), 1);
    assert!(matches!(
        index.definitions(fun)[DefinitionId::from(0)].kind(),
        DefinitionKind::SuperAssignment(_)
    ));
}

#[test]
fn test_super_assignment_nested_recorded_in_inner_scope() {
    // `x` is bound in both file and outer function. `<<-` in the inner
    // function targets the outer function scope (immediate parent), adding
    // an extra definition there.
    let index = index("x <- 0\nf <- function() { x <- 1; g <- function() { x <<- 2 } }");
    let file = ScopeId::from(0);
    let outer = ScopeId::from(1);
    let inner = ScopeId::from(2);

    // File scope: `x` has IS_BOUND (from `<-`), untouched by inner `<<-`
    let x_file = index.symbols(file).get("x").unwrap();
    assert_eq!(x_file.flags(), SymbolFlags::IS_BOUND);

    // Outer function: `x` has IS_BOUND (from both `x <- 1` and the `<<-`
    // extra definition from the inner function)
    let x_outer = index.symbols(outer).get("x").unwrap();
    assert_eq!(x_outer.flags(), SymbolFlags::IS_BOUND);

    let x_outer_defs: Vec<_> = index
        .definitions(outer)
        .iter()
        .filter(|(_, d)| index.symbols(outer).symbol(d.symbol()).name() == "x")
        .collect();
    assert_eq!(x_outer_defs.len(), 2);
    assert!(matches!(
        x_outer_defs[0].1.kind(),
        DefinitionKind::Assignment(_)
    ));
    assert!(matches!(
        x_outer_defs[1].1.kind(),
        DefinitionKind::SuperAssignment(_)
    ));

    // Inner function: `x` has IS_SUPER_BOUND (from `<<-`)
    let x_inner = index.symbols(inner).get("x").unwrap();
    assert_eq!(x_inner.flags(), SymbolFlags::IS_SUPER_BOUND);
    assert_eq!(index.definitions(inner).len(), 1);
    assert!(matches!(
        index.definitions(inner)[DefinitionId::from(0)].kind(),
        DefinitionKind::SuperAssignment(_)
    ));
}

#[test]
fn test_super_assignment_nested_skips_super_bound_scope() {
    // Both `f` and `g` use `<<-`. `f`'s `x <<- 1` marks `x` as
    // IS_SUPER_BOUND in `f` and targets the file scope. `g`'s `x <<- 2`
    // walks up from `f`, sees only IS_SUPER_BOUND (not IS_BOUND), skips it,
    // and also targets the file scope.
    let index = index("x <- 0\nf <- function() { x <<- 1; g <- function() { x <<- 2 } }");
    let file = ScopeId::from(0);
    let outer = ScopeId::from(1);
    let inner = ScopeId::from(2);

    // File scope: `x` has IS_BOUND (from `x <- 0` plus both `<<-` targets)
    let x_file = index.symbols(file).get("x").unwrap();
    assert_eq!(x_file.flags(), SymbolFlags::IS_BOUND);

    let x_file_defs: Vec<_> = index
        .definitions(file)
        .iter()
        .filter(|(_, d)| index.symbols(file).symbol(d.symbol()).name() == "x")
        .collect();
    assert_eq!(x_file_defs.len(), 3);
    assert!(matches!(
        x_file_defs[0].1.kind(),
        DefinitionKind::Assignment(_)
    ));
    assert!(matches!(
        x_file_defs[1].1.kind(),
        DefinitionKind::SuperAssignment(_)
    ));
    assert!(matches!(
        x_file_defs[2].1.kind(),
        DefinitionKind::SuperAssignment(_)
    ));

    // Outer function: `x` has IS_SUPER_BOUND only (no local `<-`)
    let x_outer = index.symbols(outer).get("x").unwrap();
    assert_eq!(x_outer.flags(), SymbolFlags::IS_SUPER_BOUND);

    // Inner function: `x` has IS_SUPER_BOUND
    let x_inner = index.symbols(inner).get("x").unwrap();
    assert_eq!(x_inner.flags(), SymbolFlags::IS_SUPER_BOUND);
}

#[test]
fn test_super_assignment_coexists_with_use_in_ancestors() {
    // `<<-` in inner function walks up from outer, finds `x` bound in file
    // scope (from `x <- 1`), so it targets the file scope -- not the outer
    // function where `x` is only used.
    let index = index("x <- 1\nf <- function() { print(x); g <- function() { x <<- 2 } }");
    let file = ScopeId::from(0);
    let outer = ScopeId::from(1);
    let inner = ScopeId::from(2);

    let x_file = index.symbols(file).get("x").unwrap();
    assert_eq!(x_file.flags(), SymbolFlags::IS_BOUND);

    // Outer function has `x` as IS_USED only (from `print(x)`). The `<<-`
    // skips it because `x` is not bound here.
    let x_outer = index.symbols(outer).get("x").unwrap();
    assert_eq!(x_outer.flags(), SymbolFlags::IS_USED);

    // Inner function has `x` as IS_SUPER_BOUND
    let x_inner = index.symbols(inner).get("x").unwrap();
    assert_eq!(x_inner.flags(), SymbolFlags::IS_SUPER_BOUND);
}

#[test]
fn test_super_assignment_not_visible_to_resolve_symbol() {
    // `<<-` creates an extra definition in the parent scope with IS_BOUND,
    // which is visible to `resolve_symbol`.
    let index = index("f <- function() { x <<- 1 }");
    let file = ScopeId::from(0);
    let fun = ScopeId::from(1);

    // File scope has `x` with IS_BOUND (extra definition from `<<-`)
    let x_file = index.symbols(file).get("x").unwrap();
    assert_eq!(x_file.flags(), SymbolFlags::IS_BOUND);

    // Function scope has `x` with IS_SUPER_BOUND
    let x = index.symbols(fun).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_SUPER_BOUND);

    // `resolve_symbol` finds `x` in the file scope via the extra definition
    let resolved = index.resolve("x", fun);
    assert!(resolved.is_some());
    assert_eq!(resolved.unwrap().0, file);
}

#[test]
fn test_super_assignment_with_use_on_value_side() {
    let index = index("f <- function() { x <<- y }");
    let fun = ScopeId::from(1);

    // `y` is a use in the function scope (where the expression lives)
    let y = index.symbols(fun).get("y").unwrap();
    assert_eq!(y.flags(), SymbolFlags::IS_USED);
}

// --- NSE / quoting constructs ---
//
// Identifiers inside `~`, `quote()`, and `bquote()` are currently recorded
// as uses. This is a known simplification; refining it is deferred as
// future work. These tests document the current behaviour.

#[test]
fn test_fixme_formula_records_uses() {
    let index = index("y ~ x + z");
    let file = ScopeId::from(0);

    assert_eq!(
        index.symbols(file).get("y").unwrap().flags(),
        SymbolFlags::IS_USED
    );
    assert_eq!(
        index.symbols(file).get("x").unwrap().flags(),
        SymbolFlags::IS_USED
    );
    assert_eq!(
        index.symbols(file).get("z").unwrap().flags(),
        SymbolFlags::IS_USED
    );
}

#[test]
fn test_fixme_one_sided_formula_records_uses() {
    let index = index("~ x + y");
    let file = ScopeId::from(0);

    assert_eq!(
        index.symbols(file).get("x").unwrap().flags(),
        SymbolFlags::IS_USED
    );
    assert_eq!(
        index.symbols(file).get("y").unwrap().flags(),
        SymbolFlags::IS_USED
    );
}

#[test]
fn test_fixme_quote_records_uses() {
    let index = index("quote(x + y)");
    let file = ScopeId::from(0);

    assert_eq!(
        index.symbols(file).get("quote").unwrap().flags(),
        SymbolFlags::IS_USED
    );
    assert_eq!(
        index.symbols(file).get("x").unwrap().flags(),
        SymbolFlags::IS_USED
    );
    assert_eq!(
        index.symbols(file).get("y").unwrap().flags(),
        SymbolFlags::IS_USED
    );
}

#[test]
fn test_fixme_quote_records_assignment() {
    let index = index("quote(x <- 1)");
    let file = ScopeId::from(0);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_BOUND);
    assert_eq!(index.definitions(file).len(), 1);
}

#[test]
fn test_fixme_formula_records_assignment() {
    let index = index("~ (x <- 1)");
    let file = ScopeId::from(0);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_BOUND);
    assert_eq!(index.definitions(file).len(), 1);
}

// --- Lambda syntax ---

#[test]
fn test_lambda_creates_scope() {
    let index = index(r"f <- \(x) x");
    let file = ScopeId::from(0);
    let fun = ScopeId::from(1);

    assert_eq!(
        index.symbols(file).get("f").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );

    assert_eq!(index.scope(fun).kind(), ScopeKind::Function);
    assert_eq!(index.scope(fun).parent(), Some(file));

    let x = index.symbols(fun).get("x").unwrap();
    assert_eq!(
        x.flags(),
        SymbolFlags::IS_BOUND
            .union(SymbolFlags::IS_PARAMETER)
            .union(SymbolFlags::IS_USED)
    );
}

#[test]
fn test_lambda_nested() {
    let index = index(r"\(x) \(y) x + y");
    let outer = ScopeId::from(1);
    let inner = ScopeId::from(2);

    let x = index.symbols(outer).get("x").unwrap();
    assert_eq!(
        x.flags(),
        SymbolFlags::IS_BOUND.union(SymbolFlags::IS_PARAMETER)
    );

    let x_inner = index.symbols(inner).get("x").unwrap();
    assert_eq!(x_inner.flags(), SymbolFlags::IS_USED);

    let y = index.symbols(inner).get("y").unwrap();
    assert_eq!(
        y.flags(),
        SymbolFlags::IS_BOUND
            .union(SymbolFlags::IS_PARAMETER)
            .union(SymbolFlags::IS_USED)
    );
}

// --- Unary expressions ---

#[test]
fn test_unary_not() {
    let index = index("!x");
    let file = ScopeId::from(0);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_USED);
}

#[test]
fn test_unary_minus() {
    let index = index("-x");
    let file = ScopeId::from(0);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_USED);
}

// --- Return, break, next ---

#[test]
fn test_return_expression() {
    let index = index("function() return(x)");
    let fun = ScopeId::from(1);

    let x = index.symbols(fun).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_USED);
}

#[test]
fn test_break_no_uses() {
    let index = index("while (TRUE) break");
    let file = ScopeId::from(0);

    // Only `TRUE` is in the tree, `break` has no identifier children
    assert_eq!(index.symbols(file).len(), 0);
    assert_eq!(index.uses(file).len(), 0);
}

#[test]
fn test_next_no_uses() {
    let index = index("while (TRUE) next");
    let file = ScopeId::from(0);

    assert_eq!(index.symbols(file).len(), 0);
    assert_eq!(index.uses(file).len(), 0);
}

// --- Pipe operator ---

#[test]
fn test_pipe_operator() {
    let index = index("x |> f()");
    let file = ScopeId::from(0);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_USED);

    let f = index.symbols(file).get("f").unwrap();
    assert_eq!(f.flags(), SymbolFlags::IS_USED);
}

// --- Chained / nested assignments ---

#[test]
fn test_chained_assignment() {
    let index = index("x <- y <- 1");
    let file = ScopeId::from(0);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_BOUND);

    let y = index.symbols(file).get("y").unwrap();
    assert_eq!(y.flags(), SymbolFlags::IS_BOUND);

    assert_eq!(index.definitions(file).len(), 2);
}

// --- Call arguments ---

#[test]
fn test_positional_call_arguments_are_uses() {
    let index = index("f(a, b)");
    let file = ScopeId::from(0);

    let f = index.symbols(file).get("f").unwrap();
    assert_eq!(f.flags(), SymbolFlags::IS_USED);

    let a = index.symbols(file).get("a").unwrap();
    assert_eq!(a.flags(), SymbolFlags::IS_USED);

    let b = index.symbols(file).get("b").unwrap();
    assert_eq!(b.flags(), SymbolFlags::IS_USED);
}

#[test]
fn test_named_call_argument_value_is_use() {
    // For `f(x = y)`, `y` should be a use but `x` should not.
    let index = index("f(x = y)");
    let file = ScopeId::from(0);

    assert!(index.symbols(file).get("x").is_none());

    let y = index.symbols(file).get("y").unwrap();
    assert_eq!(y.flags(), SymbolFlags::IS_USED);
}

#[test]
fn test_function_as_call_argument() {
    let index = index("lapply(xs, function(x) x + 1)");
    let file = ScopeId::from(0);
    let fun = ScopeId::from(1);

    let lapply = index.symbols(file).get("lapply").unwrap();
    assert_eq!(lapply.flags(), SymbolFlags::IS_USED);

    let xs = index.symbols(file).get("xs").unwrap();
    assert_eq!(xs.flags(), SymbolFlags::IS_USED);

    assert_eq!(index.scope(fun).kind(), ScopeKind::Function);
    assert_eq!(index.scope(fun).parent(), Some(file));

    let x = index.symbols(fun).get("x").unwrap();
    assert_eq!(
        x.flags(),
        SymbolFlags::IS_BOUND
            .union(SymbolFlags::IS_PARAMETER)
            .union(SymbolFlags::IS_USED)
    );
}

#[test]
fn test_nested_calls() {
    let index = index("f(g(x))");
    let file = ScopeId::from(0);

    let f = index.symbols(file).get("f").unwrap();
    assert_eq!(f.flags(), SymbolFlags::IS_USED);

    let g = index.symbols(file).get("g").unwrap();
    assert_eq!(g.flags(), SymbolFlags::IS_USED);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_USED);
}

// --- Chained extraction ---

#[test]
fn test_chained_dollar_extraction() {
    // `x$a$b` — only `x` should be a use
    let index = index("x$a$b");
    let file = ScopeId::from(0);

    assert_eq!(index.symbols(file).len(), 1);
    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_USED);
}

// --- Subset with named argument ---

#[test]
fn test_subset_named_argument_not_use() {
    // `x[drop = FALSE]` — `drop` is an argument name, not a use
    let index = index("x[drop = FALSE]");
    let file = ScopeId::from(0);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_USED);

    assert!(index.symbols(file).get("drop").is_none());
    assert_eq!(index.symbols(file).len(), 1);
}

// --- Backticked identifiers ---

#[test]
fn test_backticked_identifier_assignment() {
    let index = index("`my var` <- 1");
    let file = ScopeId::from(0);

    let sym = index.symbols(file).get("my var").unwrap();
    assert_eq!(sym.flags(), SymbolFlags::IS_BOUND);
    assert_eq!(index.definitions(file).len(), 1);
}

#[test]
fn test_backticked_identifier_use() {
    let index = index("x <- `my var`");
    let file = ScopeId::from(0);

    let sym = index.symbols(file).get("my var").unwrap();
    assert_eq!(sym.flags(), SymbolFlags::IS_USED);
}

#[test]
fn test_backticked_identifier_parameter() {
    let index = index("function(`a b`) `a b`");
    let fun = ScopeId::from(1);

    let sym = index.symbols(fun).get("a b").unwrap();
    assert_eq!(
        sym.flags(),
        SymbolFlags::IS_BOUND
            .union(SymbolFlags::IS_PARAMETER)
            .union(SymbolFlags::IS_USED)
    );
}

#[test]
fn test_backticked_for_variable() {
    let index = index("for (`i j` in xs) `i j`");
    let file = ScopeId::from(0);

    let sym = index.symbols(file).get("i j").unwrap();
    assert_eq!(
        sym.flags(),
        SymbolFlags::IS_BOUND.union(SymbolFlags::IS_USED)
    );
}

// --- String as assignment target ---

#[test]
fn test_string_assignment_target() {
    // `"x" <- 1` is equivalent to `x <- 1` in R
    let index = index("\"x\" <- 1");
    let file = ScopeId::from(0);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_BOUND);
    assert_eq!(index.definitions(file).len(), 1);
}

#[test]
fn test_string_assignment_right() {
    let index = index("1 -> \"x\"");
    let file = ScopeId::from(0);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_BOUND);
    assert_eq!(index.definitions(file).len(), 1);
}

#[test]
fn test_string_super_assignment() {
    let index = index("f <- function() { \"x\" <<- 1 }");
    let file = ScopeId::from(0);
    let fun = ScopeId::from(1);

    let x_file = index.symbols(file).get("x").unwrap();
    assert_eq!(x_file.flags(), SymbolFlags::IS_BOUND);

    let x = index.symbols(fun).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_SUPER_BOUND);
}

// --- Multiple expressions (semicolons) ---

#[test]
fn test_semicolons_multiple_expressions() {
    let index = index("x <- 1; y <- 2");
    let file = ScopeId::from(0);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_BOUND);

    let y = index.symbols(file).get("y").unwrap();
    assert_eq!(y.flags(), SymbolFlags::IS_BOUND);

    assert_eq!(index.definitions(file).len(), 2);
}

// --- Nested for loops ---

#[test]
fn test_nested_for_loops() {
    let index = index("for (i in xs) for (j in ys) f(i, j)");
    let file = ScopeId::from(0);

    let i = index.symbols(file).get("i").unwrap();
    assert_eq!(i.flags(), SymbolFlags::IS_BOUND.union(SymbolFlags::IS_USED));

    let j = index.symbols(file).get("j").unwrap();
    assert_eq!(j.flags(), SymbolFlags::IS_BOUND.union(SymbolFlags::IS_USED));

    // 2 real defs (i, j), no LoopHeader placeholders.
    assert_eq!(index.definitions(file).len(), 2);
}

// --- Assignment in loop body ---

#[test]
fn test_assignment_in_for_body() {
    let index = index("for (i in xs) x <- i");
    let file = ScopeId::from(0);

    let i = index.symbols(file).get("i").unwrap();
    assert_eq!(i.flags(), SymbolFlags::IS_BOUND.union(SymbolFlags::IS_USED));

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_BOUND);

    // 2 real defs (i, x), no LoopHeader placeholders.
    assert_eq!(index.definitions(file).len(), 2);
}

// --- File exports ---

#[test]
fn test_file_exports_simple() {
    let index = index("x <- 1\ny <- 2");
    let exports = index.exports();
    assert_eq!(exports.len(), 2);
    assert!(exports.contains_key("x"));
    assert!(exports.contains_key("y"));
}

#[test]
fn test_file_exports_excludes_nested_definitions() {
    let index = index("f <- function(x) { local_var <- x }");
    let exports = index.exports();
    assert_eq!(exports.len(), 1);
    assert!(exports.contains_key("f"));
}

#[test]
fn test_file_exports_empty() {
    let index = index("1 + 2");
    let exports = index.exports();
    assert_eq!(exports.len(), 0);
}

#[test]
fn test_file_exports_sequential_redef_keeps_last() {
    let index = index("x <- 1\nx <- 2");
    let exports = index.exports();
    // The second assignment overwrites the first, so only the last def is in
    // effect at end of file.
    assert_eq!(exports.len(), 1);
    let x = exports.get("x").unwrap();
    assert_eq!(x.len(), 1);
    // The surviving def is the second `x` (offset 7), not the first (offset 0).
    assert_eq!(x[0].1.range().start(), biome_rowan::TextSize::from(7));
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

/// Project each access into a comparable tuple via the public accessors.
fn accesses(index: &SemanticIndex) -> Vec<(&str, &str, NamespaceAccessKind, u32)> {
    index
        .namespace_accesses()
        .iter()
        .map(|access| {
            (
                access.package(),
                access.symbol(),
                access.kind(),
                access.offset().into(),
            )
        })
        .collect()
}

#[test]
fn test_namespace_access_export() {
    let index = index("dplyr::filter");
    assert_eq!(accesses(&index), [(
        "dplyr",
        "filter",
        NamespaceAccessKind::Export,
        0
    )]);
}

#[test]
fn test_namespace_access_internal() {
    let index = index("rlang:::abort");
    assert_eq!(accesses(&index), [(
        "rlang",
        "abort",
        NamespaceAccessKind::Internal,
        0
    )]);
}

#[test]
fn test_namespace_access_backtick_quoted() {
    let index = index("`my pkg`::`my fn`");
    assert_eq!(accesses(&index), [(
        "my pkg",
        "my fn",
        NamespaceAccessKind::Export,
        0
    )]);
}

#[test]
fn test_namespace_access_string_selectors() {
    let index = index("dplyr::\"filter\"");
    assert_eq!(accesses(&index), [(
        "dplyr",
        "filter",
        NamespaceAccessKind::Export,
        0
    )]);
}

#[test]
fn test_namespace_accesses_in_order() {
    let index = index("dplyr::filter\nrlang:::abort\ntidyr::pivot_longer");
    assert_eq!(accesses(&index), [
        ("dplyr", "filter", NamespaceAccessKind::Export, 0),
        ("rlang", "abort", NamespaceAccessKind::Internal, 14),
        ("tidyr", "pivot_longer", NamespaceAccessKind::Export, 28),
    ]);
}

#[test]
fn test_namespace_access_inside_call() {
    let index = index("dplyr::filter(x, y > 1)");
    assert_eq!(accesses(&index), [(
        "dplyr",
        "filter",
        NamespaceAccessKind::Export,
        0
    )]);
}

#[test]
fn test_no_namespace_accesses() {
    let index = index("x <- 1\nf(y)");
    assert!(accesses(&index).is_empty());
}

#[test]
fn test_file_exports_if_else_keeps_both_branches() {
    // Both arms of a top-level `if`/`else` could run, so both bindings are in
    // effect at end of file. exports() keeps both, in definition order.
    let index = index("if (cond) foo <- 1 else foo <- 2\nbar <- 3\n");
    let exports = index.exports();
    assert_eq!(exports.len(), 2);

    let foo = exports.get("foo").unwrap();
    let starts: Vec<biome_rowan::TextSize> = foo
        .iter()
        .map(|(_def_id, def)| def.range().start())
        .collect();
    // `foo <- 1` at offset 10, `foo <- 2` at offset 24.
    assert_eq!(starts, vec![
        biome_rowan::TextSize::from(10),
        biome_rowan::TextSize::from(24),
    ]);
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
        effects_registry::lookup("base", name).copied()
    }
}

/// Returns per-path resolutions; unknown paths yield `None`.
struct MapResolver(std::collections::HashMap<String, SourceResolution>);

impl ImportsResolver for MapResolver {
    fn resolve_source(&mut self, path: &str) -> Option<SourceResolution> {
        self.0.get(path).cloned()
    }

    fn resolve_effects(&mut self, name: &str, _: &[String], _: bool) -> Option<EffectsHandlers> {
        effects_registry::lookup("base", name).copied()
    }
}

/// A source handler that resolves one call to a fixed collation of files,
/// standing in for a collation-style callee. Attached to the `source` name
/// (which passes the `is_annotated` front gate) by [`MultiFileResolver`].
#[derive(Debug)]
struct CollationHandler;

static COLLATION_HANDLER: CollationHandler = CollationHandler;

impl EffectHandler for CollationHandler {
    type Output = Vec<String>;

    fn resolve(&self, _call: &RCall, _ctx: &CallContext) -> Option<Vec<String>> {
        Some(vec!["a.R".into(), "b.R".into()])
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
            });
        }
        effects_registry::lookup("base", name).copied()
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
            });
        }
        None
    }
}

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
