use aether_parser::parse;
use aether_parser::RParserOptions;
use aether_syntax::RSyntaxKind;
use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_index::semantic_index;
use oak_index::semantic_index::DefinitionId;
use oak_index::semantic_index::DefinitionKind;
use oak_index::semantic_index::DirectiveKind;
use oak_index::semantic_index::ScopeId;
use oak_index::semantic_index::ScopeKind;
use oak_index::semantic_index::SemanticIndex;
use oak_index::semantic_index::SymbolFlags;
use oak_index::semantic_index::UseId;
use oak_index::semantic_index_with_source_resolver;
use oak_index::SourceResolution;
use url::Url;

fn index(source: &str) -> SemanticIndex {
    let parsed = parse(source, RParserOptions::default());

    if parsed.has_error() {
        panic!("source has syntax errors: {source}");
    }

    semantic_index(&parsed.tree())
}

fn directive_kinds(index: &SemanticIndex) -> Vec<&DirectiveKind> {
    index.file_directives().iter().map(|d| d.kind()).collect()
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
    // At file scope, `<<-` targets the file scope itself (no parent to
    // walk to), so the symbol gets both IS_SUPER_BOUND and IS_BOUND.
    let index = index("x <<- 1");
    let file = ScopeId::from(0);

    let x = index.symbols(file).get("x").unwrap();
    assert_eq!(
        x.flags(),
        SymbolFlags::IS_SUPER_BOUND.union(SymbolFlags::IS_BOUND)
    );

    // Two definitions: one from the current-scope recording, one from the
    // target-scope recording (same scope in this case).
    assert_eq!(index.definitions(file).len(), 2);
    assert!(matches!(
        index.definitions(file)[DefinitionId::from(0)].kind(),
        DefinitionKind::SuperAssignment(_)
    ));
    assert!(matches!(
        index.definitions(file)[DefinitionId::from(1)].kind(),
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

    assert_eq!(index.definitions(file).len(), 2);
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
    let resolved = index.resolve_symbol("x", fun);
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
    let exports = index.file_exports();
    assert_eq!(exports.len(), 2);
    assert_eq!(exports[0].0, "x");
    assert_eq!(exports[1].0, "y");
}

#[test]
fn test_file_exports_excludes_nested_definitions() {
    let index = index("f <- function(x) { local_var <- x }");
    let exports = index.file_exports();
    assert_eq!(exports.len(), 1);
    assert_eq!(exports[0].0, "f");
}

#[test]
fn test_file_exports_empty() {
    let index = index("1 + 2");
    let exports = index.file_exports();
    assert_eq!(exports.len(), 0);
}

#[test]
fn test_file_exports_multiple_defs_same_symbol() {
    let index = index("x <- 1\nx <- 2");
    let exports = index.file_exports();
    // Both definition sites are returned
    assert_eq!(exports.len(), 2);
    assert_eq!(exports[0].0, "x");
    assert_eq!(exports[1].0, "x");
}

// --- File directives ---

#[test]
fn test_directive_library_identifier() {
    let index = index("library(dplyr)");
    assert_eq!(directive_kinds(&index), [&DirectiveKind::Attach(
        "dplyr".into()
    )]);
}

#[test]
fn test_directive_library_string() {
    let index = index("library(\"tidyr\")");
    assert_eq!(directive_kinds(&index), [&DirectiveKind::Attach(
        "tidyr".into()
    )]);
}

#[test]
fn test_directive_library_single_quoted_string() {
    let index = index("library('ggplot2')");
    assert_eq!(directive_kinds(&index), [&DirectiveKind::Attach(
        "ggplot2".into()
    )]);
}

#[test]
fn test_directive_require() {
    let index = index("require(data.table)");
    assert_eq!(directive_kinds(&index), [&DirectiveKind::Attach(
        "data.table".into()
    )]);
}

#[test]
fn test_directive_multiple_libraries() {
    let index = index("library(dplyr)\nlibrary(tidyr)\nrequire(ggplot2)");
    assert_eq!(directive_kinds(&index), [
        &DirectiveKind::Attach("dplyr".into()),
        &DirectiveKind::Attach("tidyr".into()),
        &DirectiveKind::Attach("ggplot2".into()),
    ]);
}

#[test]
fn test_directive_named_argument_ignored() {
    let index = index("library(package = dplyr)");
    assert_eq!(directive_kinds(&index), Vec::<&DirectiveKind>::new());
}

#[test]
fn test_directive_multiple_arguments_ignored() {
    let index = index("library(dplyr, warn.conflicts = FALSE)");
    assert_eq!(directive_kinds(&index), Vec::<&DirectiveKind>::new());
}

#[test]
fn test_directive_no_arguments_ignored() {
    let index = index("library()");
    assert_eq!(directive_kinds(&index), Vec::<&DirectiveKind>::new());
}

#[test]
fn test_directive_library_in_function_scope() {
    // library() in a function body now records a scoped directive
    let index = index("f <- function() { library(dplyr) }");
    assert_eq!(directive_kinds(&index), [&DirectiveKind::Attach(
        "dplyr".into()
    )]);
    let directives = index.file_directives();
    assert_ne!(directives[0].scope(), ScopeId::from(0));
}

#[test]
fn test_directive_non_static_argument_ignored() {
    let index = index("library(get_pkg())");
    assert_eq!(directive_kinds(&index), Vec::<&DirectiveKind>::new());
}

#[test]
fn test_directive_preserves_offset() {
    let index = index("x <- 1\nlibrary(dplyr)");
    let directives = index.file_directives();
    assert_eq!(directives.len(), 1);
    assert_eq!(directives[0].offset(), biome_rowan::TextSize::from(7));
}

// --- source() directives ---

#[test]
fn test_directive_source_no_resolver() {
    // Without a resolver, source() produces no directives
    let index = index("source(\"helpers.R\")");
    assert_eq!(directive_kinds(&index), Vec::<&DirectiveKind>::new());
}

#[test]
fn test_directive_source_single_quoted_no_resolver() {
    let index = index("source('utils/helpers.R')");
    assert_eq!(directive_kinds(&index), Vec::<&DirectiveKind>::new());
}

#[test]
fn test_directive_source_identifier_ignored() {
    let index = index("source(my_file)");
    assert_eq!(directive_kinds(&index), Vec::<&DirectiveKind>::new());
}

#[test]
fn test_directive_source_non_static_argument_ignored() {
    let index = index("source(paste0(\"path/\", name))");
    assert_eq!(directive_kinds(&index), Vec::<&DirectiveKind>::new());
}

#[test]
fn test_directive_source_named_argument_ignored() {
    let index = index("source(file = \"helpers.R\")");
    assert_eq!(directive_kinds(&index), Vec::<&DirectiveKind>::new());
}

#[test]
fn test_directive_source_multiple_arguments_ignored() {
    let index = index("source(\"helpers.R\", local = TRUE)");
    assert_eq!(directive_kinds(&index), Vec::<&DirectiveKind>::new());
}

#[test]
fn test_directive_source_no_arguments_ignored() {
    let index = index("source()");
    assert_eq!(directive_kinds(&index), Vec::<&DirectiveKind>::new());
}

#[test]
fn test_directive_source_not_at_file_scope() {
    let index = index("f <- function() { source(\"helpers.R\") }");
    assert_eq!(directive_kinds(&index), Vec::<&DirectiveKind>::new());
}

#[test]
fn test_directive_source_no_resolver_no_directives() {
    let index = index("x <- 1\nsource(\"helpers.R\")");
    let directives = index.file_directives();
    assert_eq!(directives.len(), 0);
}

#[test]
fn test_directive_source_mixed_with_library() {
    let index = index("library(dplyr)\nsource(\"helpers.R\")\nlibrary(tidyr)");
    assert_eq!(directive_kinds(&index), [
        &DirectiveKind::Attach("dplyr".into()),
        &DirectiveKind::Attach("tidyr".into()),
    ]);
}

// --- declare() directives ---

#[test]
fn test_directive_declare_source_no_resolver() {
    let index = index("declare(source(\"helpers.R\"))");
    assert_eq!(directive_kinds(&index), Vec::<&DirectiveKind>::new());
}

#[test]
fn test_directive_declare_source_single_quotes_no_resolver() {
    let index = index("declare(source('utils.R'))");
    assert_eq!(directive_kinds(&index), Vec::<&DirectiveKind>::new());
}

#[test]
fn test_directive_tilde_declare_source_no_resolver() {
    let index = index("~declare(source(\"helpers.R\"))");
    assert_eq!(directive_kinds(&index), Vec::<&DirectiveKind>::new());
}

#[test]
fn test_fixme_directive_declare_library_transparent() {
    // `declare()` is transparent: the inner `library(dplyr)` is still
    // picked up as a directive.
    // FIXME: We should declare `declare()` as a quoting function.
    let index = index("declare(library(dplyr))");
    assert_eq!(directive_kinds(&index), [&DirectiveKind::Attach(
        "dplyr".into()
    )]);
}

#[test]
fn test_directive_declare_not_at_file_scope() {
    let index = index("f <- function() { declare(source(\"helpers.R\")) }");
    assert_eq!(directive_kinds(&index), Vec::<&DirectiveKind>::new());
}

#[test]
fn test_directive_tilde_declare_not_at_file_scope() {
    let index = index("f <- function() { ~declare(source(\"helpers.R\")) }");
    assert_eq!(directive_kinds(&index), Vec::<&DirectiveKind>::new());
}

#[test]
fn test_directive_declare_mixed_with_bare() {
    let index = index("library(dplyr)\ndeclare(source(\"helpers.R\"))\nsource(\"utils.R\")");
    assert_eq!(directive_kinds(&index), [&DirectiveKind::Attach(
        "dplyr".into()
    ),]);
}

#[test]
fn test_directive_declare_source_no_resolver_no_directives() {
    let index = index("x <- 1\ndeclare(source(\"helpers.R\"))");
    let directives = index.file_directives();
    assert_eq!(directives.len(), 0);
}

#[test]
fn test_directive_tilde_declare_source_no_resolver_no_directives() {
    let index = index("x <- 1\n~declare(source(\"helpers.R\"))");
    let directives = index.file_directives();
    assert_eq!(directives.len(), 0);
}

#[test]
fn test_directive_declare_non_call_arg_ignored() {
    let index = index("declare(42)");
    assert_eq!(directive_kinds(&index), Vec::<&DirectiveKind>::new());
}

#[test]
fn test_directive_declare_identifier_source_arg_ignored() {
    let index = index("declare(source(my_file))");
    assert_eq!(directive_kinds(&index), Vec::<&DirectiveKind>::new());
}

// --- source() with resolver ---

fn index_with_resolver(
    source: &str,
    resolver: impl FnMut(&str) -> Option<SourceResolution>,
) -> SemanticIndex {
    let parsed = parse(source, RParserOptions::default());
    if parsed.has_error() {
        panic!("source has syntax errors: {source}");
    }
    semantic_index_with_source_resolver(&parsed.tree(), resolver)
}

fn helper_resolution() -> SourceResolution {
    SourceResolution {
        definitions: vec![(
            "helper".into(),
            Url::parse("file:///test/helpers.R").unwrap(),
            TextRange::new(TextSize::from(0), TextSize::from(6)),
        )],
        packages: vec![],
    }
}

#[test]
fn test_source_resolver_injects_definitions() {
    let code = "source(\"helpers.R\")\nhelper\n";
    let index = index_with_resolver(code, |_| Some(helper_resolution()));
    let file = ScopeId::from(0);

    // The use of `helper` resolves to the sourced definition
    let map = index.use_def_map(file);
    // Use 0 is `source`, use 1 is `helper`
    let bindings = map.bindings_at_use(UseId::from(1));
    assert!(!bindings.definitions().is_empty());

    let def_id = bindings.definitions()[0];
    let def = &index.definitions(file)[def_id];
    let DefinitionKind::Sourced { file: ref url } = def.kind() else {
        panic!("expected Sourced definition, got {:?}", def.kind());
    };
    assert_eq!(url.as_str(), "file:///test/helpers.R");

    // file_exports() excludes sourced definitions
    let exports = index.file_exports();
    assert!(!exports.iter().any(|(name, _)| *name == "helper"));

    // file_all_definitions() includes sourced definitions
    let own_url = Url::parse("file:///test/main.R").unwrap();
    let all_defs = index.file_all_definitions(&own_url);
    let sourced = all_defs
        .iter()
        .find(|(name, _, _)| *name == "helper")
        .unwrap();
    assert_eq!(sourced.1.as_str(), "file:///test/helpers.R");
}

#[test]
fn test_source_resolver_offset_visibility() {
    let code = "helper\nsource(\"helpers.R\")\nhelper\n";
    let index = index_with_resolver(code, |_| Some(helper_resolution()));
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
    assert!(matches!(def.kind(), DefinitionKind::Sourced { .. }));
}

#[test]
fn test_source_resolver_in_function_scope() {
    let code = "f <- function() {\n  source(\"helpers.R\")\n  helper\n}\nhelper\n";
    let index = index_with_resolver(code, |_| Some(helper_resolution()));
    let fun = ScopeId::from(1);
    let file = ScopeId::from(0);

    // `helper` inside the function resolves to the sourced definition
    // Function scope uses: source(0), helper(1)
    let fun_map = index.use_def_map(fun);
    let inner_bindings = fun_map.bindings_at_use(UseId::from(1));
    assert!(!inner_bindings.definitions().is_empty());
    let def_id = inner_bindings.definitions()[0];
    let def = &index.definitions(fun)[def_id];
    assert!(matches!(def.kind(), DefinitionKind::Sourced { .. }));

    // `helper` at file scope does not resolve
    let file_map = index.use_def_map(file);
    let outer_bindings = file_map.bindings_at_use(UseId::from(0));
    assert!(outer_bindings.may_be_unbound());
    assert!(outer_bindings.definitions().is_empty());
}

#[test]
fn test_source_resolver_packages_become_directives() {
    let code = "source(\"helpers.R\")\n";
    let index = index_with_resolver(code, |_| {
        Some(SourceResolution {
            definitions: vec![],
            packages: vec!["dplyr".into()],
        })
    });

    assert_eq!(directive_kinds(&index), [&DirectiveKind::Attach(
        "dplyr".into()
    )]);
}

#[test]
fn test_source_resolver_later_shadows_earlier() {
    let code = "source(\"a.R\")\nsource(\"b.R\")\nfoo\n";
    let parsed = parse(code, RParserOptions::default());

    let a_url = Url::parse("file:///test/a.R").unwrap();
    let b_url = Url::parse("file:///test/b.R").unwrap();
    let a_url_clone = a_url.clone();
    let b_url_clone = b_url.clone();

    let index = semantic_index_with_source_resolver(&parsed.tree(), move |path| {
        let (url, range) = match path {
            "a.R" => (
                a_url_clone.clone(),
                TextRange::new(TextSize::from(0), TextSize::from(3)),
            ),
            "b.R" => (
                b_url_clone.clone(),
                TextRange::new(TextSize::from(0), TextSize::from(3)),
            ),
            _ => return None,
        };
        Some(SourceResolution {
            definitions: vec![("foo".to_string(), url, range)],
            packages: Vec::new(),
        })
    });

    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // Uses: source(0), source(1), foo(2)
    let bindings = map.bindings_at_use(UseId::from(2));
    assert_eq!(bindings.definitions().len(), 1);

    let def_id = bindings.definitions()[0];
    let def = &index.definitions(file)[def_id];
    let DefinitionKind::Sourced { file: ref url } = def.kind() else {
        panic!("expected Sourced definition, got {:?}", def.kind());
    };
    assert_eq!(*url, b_url);
}
