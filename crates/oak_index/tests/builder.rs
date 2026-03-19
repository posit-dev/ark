use aether_parser::parse;
use aether_parser::RParserOptions;
use oak_index::builder::build;
use oak_index::semantic_index::BindingId;
use oak_index::semantic_index::ScopeId;
use oak_index::semantic_index::ScopeKind;
use oak_index::semantic_index::SemanticIndex;
use oak_index::semantic_index::SymbolFlags;
use oak_index::semantic_index::UseId;

fn index(source: &str) -> SemanticIndex {
    let parsed = parse(source, RParserOptions::default());
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

    assert_eq!(index.bindings(file).len(), 1);
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

    assert_eq!(index.bindings(file).len(), 1);
    assert_eq!(index.uses(file).len(), 1);
}

#[test]
fn test_rhs_collected_before_lhs() {
    // The first use site should be `y` (RHS) and the first binding site should be `x` (LHS).
    let index = index("x <- y");
    let file = ScopeId::from(0);

    let use_site = &index.uses(file)[UseId::from(0)];
    let use_sym = index.symbols(file).symbol(use_site.symbol());
    assert_eq!(use_sym.name(), "y");

    let bind_site = &index.bindings(file)[BindingId::from(0)];
    let bind_sym = index.symbols(file).symbol(bind_site.symbol());
    assert_eq!(bind_sym.name(), "x");
}

#[test]
fn test_multiple_assignments_same_symbol() {
    let index = index("x <- 1\nx <- 2");
    let file = ScopeId::from(0);

    // One symbol, two binding sites
    assert_eq!(index.symbols(file).len(), 1);
    assert_eq!(index.bindings(file).len(), 2);
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

    assert_eq!(index.bindings(fun_scope).len(), 1);
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

    // Only `f` is recorded (as a use), `x` is an argument name, not a binding
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

    assert_eq!(index.bindings(file).len(), 0);
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

    assert_eq!(index.bindings(fun).len(), 2);
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
    assert_eq!(index.bindings(file).len(), 1);
}

#[test]
fn test_arrow_assignment_in_call_argument() {
    // `<-` in a call argument still creates a binding in the enclosing scope
    let index = index("f(x <- 1)");
    let file = ScopeId::from(0);

    assert!(index.symbols(file).get("x").is_some());
    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_BOUND);
    assert_eq!(index.bindings(file).len(), 1);
}

#[test]
fn test_equals_in_call_argument_still_not_assignment() {
    // `=` in a call argument is NOT an assignment (unchanged)
    let index = index("f(x = 1)");
    let file = ScopeId::from(0);

    assert!(index.symbols(file).get("x").is_none());
    assert_eq!(index.bindings(file).len(), 0);
}

#[test]
fn test_parenthesized_arrow_assignment() {
    // `<-` is always an assignment, even inside parentheses
    let index = index("(x <- 1)");
    let file = ScopeId::from(0);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(x.flags(), SymbolFlags::IS_BOUND);
    assert_eq!(index.bindings(file).len(), 1);
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
    assert_eq!(index.bindings(file).len(), 1);
}
