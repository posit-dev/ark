use aether_parser::parse;
use aether_parser::RParserOptions;
use aether_syntax::RSyntaxKind;
use oak_index::builder::build;
use oak_index::semantic_index::DefinitionId;
use oak_index::semantic_index::DefinitionKind;
use oak_index::semantic_index::ScopeId;
use oak_index::semantic_index::ScopeKind;
use oak_index::semantic_index::SemanticIndex;
use oak_index::semantic_index::SymbolFlags;
use oak_index::semantic_index::UseId;

fn index(source: &str) -> SemanticIndex {
    let parsed = parse(source, RParserOptions::default());

    if parsed.has_error() {
        panic!("source has syntax errors: {source}");
    }

    build(&parsed.tree())
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
    assert_eq!(node.kind(), RSyntaxKind::R_BINARY_EXPRESSION);
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
    assert_eq!(node.kind(), RSyntaxKind::R_PARAMETER);
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
    let (scope, _) = index.resolve_symbol("y", inner).unwrap();
    assert_eq!(scope, inner);

    // `x` resolves in the file scope
    let file = ScopeId::from(0);
    let (scope, _) = index.resolve_symbol("x", inner).unwrap();
    assert_eq!(scope, file);

    // `z` doesn't resolve
    assert!(index.resolve_symbol("z", inner).is_none());
}

#[test]
fn test_resolve_prefers_inner_scope() {
    let index = index("x <- 1\nf <- function(x) x");
    let file = ScopeId::from(0);
    let fun = ScopeId::from(1);

    // From function scope, `x` resolves to the function's own `x`
    let (scope, _) = index.resolve_symbol("x", fun).unwrap();
    assert_eq!(scope, fun);

    // From file scope, `x` resolves to file's `x`
    let (scope, _) = index.resolve_symbol("x", file).unwrap();
    assert_eq!(scope, file);
}

#[test]
fn test_scope_at() {
    let source = "x <- 1\nf <- function(y) y";
    let idx = index(source);
    let file = ScopeId::from(0);
    let fun = ScopeId::from(1);

    // Offset 0 is in `x` -- file scope
    assert_eq!(idx.scope_at(biome_rowan::TextSize::from(0)), file);

    // Offset inside the function body
    let body_offset = source.find(") y").unwrap() + 2;
    assert_eq!(
        idx.scope_at(biome_rowan::TextSize::from(body_offset as u32)),
        fun
    );
}

#[test]
fn test_child_scopes() {
    let index = index("f <- function(x) x\ng <- function(y) y");
    let file = ScopeId::from(0);

    let children: Vec<_> = index.child_scopes(file).collect();
    assert_eq!(children.len(), 2);
}

#[test]
fn test_ancestor_scopes() {
    let index = index("f <- function(x) function(y) y");
    let inner = ScopeId::from(2);
    let outer = ScopeId::from(1);
    let file = ScopeId::from(0);

    let ancestors: Vec<_> = index.ancestor_scopes(inner).collect();
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
    assert_eq!(node.kind(), RSyntaxKind::R_FOR_STATEMENT);
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
    let index = index("dplyr::filter");
    let file = ScopeId::from(0);

    assert!(index.symbols(file).get("dplyr").is_none());
    assert!(index.symbols(file).get("filter").is_none());
    assert_eq!(index.symbols(file).len(), 0);
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
    // At file scope, `<<-` records in file scope with IS_SUPER_BOUND
    let index = index("x <<- 1");
    let file = ScopeId::from(0);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_SUPER_BOUND);

    assert_eq!(index.definitions(file).len(), 1);
    let DefinitionKind::SuperAssignment(node) =
        index.definitions(file)[DefinitionId::from(0)].kind()
    else {
        panic!("expected SuperAssignment");
    };
    assert_eq!(node.kind(), RSyntaxKind::R_BINARY_EXPRESSION);
    assert_eq!(index.uses(file).len(), 0);
}

#[test]
fn test_super_assignment_right_at_file_scope() {
    let index = index("1 ->> x");
    let file = ScopeId::from(0);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_SUPER_BOUND);

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
    // appears, not in an ancestor scope.
    let index = index("f <- function() { x <<- 1 }");
    let file = ScopeId::from(0);
    let fun = ScopeId::from(1);

    // File scope only has `f`
    assert!(index.symbols(file).get("x").is_none());
    assert_eq!(index.definitions(file).len(), 1);
    assert!(matches!(
        index.definitions(file)[DefinitionId::from(0)].kind(),
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

    assert!(index.symbols(file).get("x").is_none());

    let x = index.symbols(fun).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_SUPER_BOUND);
}

#[test]
fn test_super_assignment_does_not_pollute_ancestor() {
    // `x <- 1` is in file scope, `x <<- 2` is in the function. The `<<-`
    // does NOT add a definition to the file scope.
    let index = index("x <- 1\nf <- function() { x <<- 2 }");
    let file = ScopeId::from(0);
    let fun = ScopeId::from(1);

    // File scope: `x` has IS_BOUND from the `<-`, `f` has IS_BOUND
    let x_file = index.symbols(file).get("x").unwrap();
    assert_eq!(x_file.flags(), SymbolFlags::IS_BOUND);

    let x_file_defs: Vec<_> = index
        .definitions(file)
        .iter()
        .filter(|(_, d)| index.symbols(file).symbol(d.symbol()).name() == "x")
        .collect();
    assert_eq!(x_file_defs.len(), 1);
    assert!(matches!(
        x_file_defs[0].1.kind(),
        DefinitionKind::Assignment(_)
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
    // function records the definition in the inner scope, not in any ancestor.
    let index = index("x <- 0\nf <- function() { x <- 1; g <- function() { x <<- 2 } }");
    let file = ScopeId::from(0);
    let outer = ScopeId::from(1);
    let inner = ScopeId::from(2);

    // File scope: `x` has IS_BOUND (from `<-`), untouched by inner `<<-`
    let x_file = index.symbols(file).get("x").unwrap();
    assert_eq!(x_file.flags(), SymbolFlags::IS_BOUND);

    // Outer function: `x` has IS_BOUND (from `<-`), no super-assignment here
    let x_outer = index.symbols(outer).get("x").unwrap();
    assert_eq!(x_outer.flags(), SymbolFlags::IS_BOUND);

    let x_outer_defs: Vec<_> = index
        .definitions(outer)
        .iter()
        .filter(|(_, d)| index.symbols(outer).symbol(d.symbol()).name() == "x")
        .collect();
    assert_eq!(x_outer_defs.len(), 1);
    assert!(matches!(
        x_outer_defs[0].1.kind(),
        DefinitionKind::Assignment(_)
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
fn test_super_assignment_coexists_with_use_in_ancestors() {
    // Outer function uses `x` but doesn't bind it. `<<-` in the inner
    // function records in the inner scope. Ancestors are unaffected.
    let index = index("x <- 1\nf <- function() { print(x); g <- function() { x <<- 2 } }");
    let file = ScopeId::from(0);
    let outer = ScopeId::from(1);
    let inner = ScopeId::from(2);

    let x_file = index.symbols(file).get("x").unwrap();
    assert_eq!(x_file.flags(), SymbolFlags::IS_BOUND);

    // Outer function has `x` as `IS_USED` only (from `print(x)`)
    let x_outer = index.symbols(outer).get("x").unwrap();
    assert_eq!(x_outer.flags(), SymbolFlags::IS_USED);

    // Inner function has `x` as IS_SUPER_BOUND
    let x_inner = index.symbols(inner).get("x").unwrap();
    assert_eq!(x_inner.flags(), SymbolFlags::IS_SUPER_BOUND);
}

#[test]
fn test_super_assignment_not_visible_to_resolve_symbol() {
    // `<<-` does not create a binding visible to `resolve_symbol` (which
    // checks IS_BOUND, not IS_SUPER_BOUND). Cross-scope effects of `<<-`
    // are a runtime concern, not statically modelled.
    let index = index("f <- function() { x <<- 1 }");
    let file = ScopeId::from(0);
    let fun = ScopeId::from(1);

    // File scope has no `x`
    assert!(index.symbols(file).get("x").is_none());

    // Function scope has `x` with IS_SUPER_BOUND
    let x = index.symbols(fun).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_SUPER_BOUND);

    // resolve_symbol does not find `x` because no scope has IS_BOUND for it
    assert!(index.resolve_symbol("x", fun).is_none());
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

    assert!(index.symbols(file).get("x").is_none());

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

    assert_eq!(index.definitions(file).len(), 2);
}
