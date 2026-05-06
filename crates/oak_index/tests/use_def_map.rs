use aether_parser::parse;
use aether_parser::RParserOptions;
use oak_index::semantic_index;
use oak_index::semantic_index::DefinitionId;
use oak_index::semantic_index::ScopeId;
use oak_index::semantic_index::SemanticIndex;
use oak_index::semantic_index::UseId;
use stdext::assert_not;

fn index(source: &str) -> SemanticIndex {
    let parsed = parse(source, RParserOptions::default());
    semantic_index(
        &parsed.tree(),
        &url::Url::parse("file:///test/test.R").unwrap(),
    )
}

// --- Straight-line code ---

#[test]
fn test_single_def_single_use() {
    let index = index("x <- 1\nx");
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    let bindings = map.bindings_at_use(UseId::from(0));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(0)]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_use_before_def_is_unbound() {
    let index = index("x\nx <- 1");
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    let bindings = map.bindings_at_use(UseId::from(0));
    assert!(bindings.definitions().is_empty());
    assert!(bindings.may_be_unbound());
}

#[test]
fn test_second_def_shadows_first() {
    let index = index("x <- 1\nx <- 2\nx");
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // The use of `x` should see only the second definition.
    let bindings = map.bindings_at_use(UseId::from(0));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(1)]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_use_between_defs() {
    let index = index("x <- 1\nx\nx <- 2\nx");
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // First use sees def 0, second use sees def 1.
    let use0 = map.bindings_at_use(UseId::from(0));
    assert_eq!(use0.definitions(), &[DefinitionId::from(0)]);
    assert_not!(use0.may_be_unbound());

    let use1 = map.bindings_at_use(UseId::from(1));
    assert_eq!(use1.definitions(), &[DefinitionId::from(1)]);
    assert_not!(use1.may_be_unbound());
}

#[test]
fn test_rhs_use_sees_previous_binding() {
    let index = index("x <- 1\nx <- x + 1\nx");
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // `x` on the RHS of the second assignment (use 0)
    let rhs_use = map.bindings_at_use(UseId::from(0));
    assert_eq!(rhs_use.definitions(), &[DefinitionId::from(0)]);

    // Final `x` (use 1)
    let final_use = map.bindings_at_use(UseId::from(1));
    assert_eq!(final_use.definitions(), &[DefinitionId::from(1)]);
}

#[test]
fn test_different_symbols_independent() {
    let index = index("x <- 1\ny <- 2\nx\ny");
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    let use_x = map.bindings_at_use(UseId::from(0));
    assert_eq!(use_x.definitions(), &[DefinitionId::from(0)]);
    assert_not!(use_x.may_be_unbound());

    let use_y = map.bindings_at_use(UseId::from(1));
    assert_eq!(use_y.definitions(), &[DefinitionId::from(1)]);
    assert_not!(use_y.may_be_unbound());
}

#[test]
fn test_unbound_symbol() {
    let index = index("x");
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    let bindings = map.bindings_at_use(UseId::from(0));
    assert!(bindings.definitions().is_empty());
    assert!(bindings.may_be_unbound());
}

// --- If/else ---

#[test]
fn test_if_else_both_branches_define() {
    let index = index(
        "\
x <- 1       # def 0
if (cond) {
    x <- 2   # def 1
} else {
    x <- 3   # def 2
}
x            # use 1 -> {def 1, def 2}
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // Uses: `cond` is use 0, final `x` is use 1.
    let bindings = map.bindings_at_use(UseId::from(1));
    assert_eq!(bindings.definitions(), &[
        DefinitionId::from(1),
        DefinitionId::from(2)
    ]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_if_only_one_branch_defines() {
    let index = index(
        "\
x <- 1       # def 0
if (cond) {
    x <- 2   # def 1
}
x            # use 1 -> {def 0, def 1}
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // Uses: `cond` is use 0, final `x` is use 1.
    let bindings = map.bindings_at_use(UseId::from(1));
    assert_eq!(bindings.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(1)
    ]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_if_no_prior_def_one_branch() {
    let index = index(
        "\
if (cond) {
    x <- 1   # def 0
}
x            # use 1 -> {def 0}, may_be_unbound
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // Uses: `cond` is use 0, final `x` is use 1.
    let bindings = map.bindings_at_use(UseId::from(1));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(0)]);
    assert!(bindings.may_be_unbound());
}

#[test]
fn test_if_no_prior_def_both_branches() {
    let index = index(
        "\
if (cond) {
    x <- 1   # def 0
} else {
    x <- 2   # def 1
}
x            # use 1 -> {def 0, def 1}, not unbound
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // Uses: `cond` is use 0, final `x` is use 1.
    let bindings = map.bindings_at_use(UseId::from(1));
    assert_eq!(bindings.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(1)
    ]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_if_condition_uses_see_pre_if_state() {
    let index = index("x <- 1\nif (x) {}");
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // Use of `x` in condition sees def 0.
    let bindings = map.bindings_at_use(UseId::from(0));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(0)]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_nested_if_else() {
    let index = index(
        "\
if (c1) {
    if (c2) {
        x <- 1   # def 0
    } else {
        x <- 2   # def 1
    }
} else {
    x <- 3       # def 2
}
x                # use 2 -> {def 0, def 1, def 2}
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // Uses: c1 is use 0, c2 is use 1, final x is use 2.
    let bindings = map.bindings_at_use(UseId::from(2));
    assert_eq!(bindings.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(1),
        DefinitionId::from(2)
    ]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_if_else_without_braces() {
    let index = index("if (cond) x <- 1 else x <- 2\nx");
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // Uses: `cond` is use 0, final `x` is use 1.
    let bindings = map.bindings_at_use(UseId::from(1));
    assert_eq!(bindings.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(1)
    ]);
    assert_not!(bindings.may_be_unbound());
}

// --- For loops ---

#[test]
fn test_for_variable_is_definite() {
    let index = index("for (i in xs) {}\ni");
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // `i` is always bound (R sets to NULL for empty sequences).
    // Uses: `xs` is use 0, `i` is use 1.
    let bindings = map.bindings_at_use(UseId::from(1));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(0)]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_for_body_assignment_is_conditional() {
    let index = index(
        "\
for (i in xs) { # def 0 (i)
    x <- 1      # def 1
}
x               # use 1 -> {def 1}, may_be_unbound (body may not execute)
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // Uses: `xs` is use 0, final `x` is use 1.
    let bindings = map.bindings_at_use(UseId::from(1));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(1)]);
    assert!(bindings.may_be_unbound());
}

#[test]
fn test_for_body_assignment_merges_with_pre_loop() {
    let index = index(
        "\
x <- 0          # def 0
for (i in xs) { # def 1 (i)
    x <- 1      # def 2
}
x               # use 1 -> {def 0, def 2}
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // Uses: `xs` is use 0, final `x` is use 1.
    let bindings = map.bindings_at_use(UseId::from(1));
    assert_eq!(bindings.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(2)
    ]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_for_variable_used_inside_body() {
    let index = index(
        "\
for (i in xs) {
    print(i)    # use of `i` inside body sees for-variable def
}
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // Uses: `xs` is use 0, `print` is use 1, `i` is use 2.
    let bindings = map.bindings_at_use(UseId::from(2));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(0)]);
    assert_not!(bindings.may_be_unbound());
}

// --- While loops ---

#[test]
fn test_while_body_is_conditional() {
    let index = index(
        "\
while (cond) {
    x <- 1     # def 0
}
x              # use 1 -> {def 0}, may_be_unbound
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // Uses: `cond` is use 0, final `x` is use 1.
    let bindings = map.bindings_at_use(UseId::from(1));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(0)]);
    assert!(bindings.may_be_unbound());
}

#[test]
fn test_while_merges_with_pre_loop() {
    let index = index(
        "\
x <- 0         # def 0
while (cond) {
    x <- 1     # def 1
}
x              # use 1 -> {def 0, def 1}
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // Uses: `cond` is use 0, final `x` is use 1.
    let bindings = map.bindings_at_use(UseId::from(1));
    assert_eq!(bindings.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(1)
    ]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_while_condition_use() {
    let index = index("x <- 1\nwhile (x) {}");
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // Condition use sees def 0.
    let bindings = map.bindings_at_use(UseId::from(0));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(0)]);
    assert_not!(bindings.may_be_unbound());
}

// --- Repeat loops ---

#[test]
fn test_repeat_body_is_definite() {
    let index = index(
        "\
repeat {
    x <- 1   # def 0
    break
}
x            # use 0 -> {def 0}, not unbound
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    let bindings = map.bindings_at_use(UseId::from(0));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(0)]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_repeat_shadows_prior_def() {
    let index = index(
        "\
x <- 0       # def 0
repeat {
    x <- 1   # def 1
    break
}
x            # use 0 -> {def 1} (repeat always executes)
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    let bindings = map.bindings_at_use(UseId::from(0));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(1)]);
    assert_not!(bindings.may_be_unbound());
}

// --- Loop-carried definitions ---
//
// Uses at the top of a loop body can see definitions from the bottom
// (from a previous iteration). The builder pre-allocates placeholder
// definitions from the pre-scan before walking the body. The
// placeholder shares its `DefinitionId` with the real definition, so
// uses at the top see the same ID that the actual assignment produces.

#[test]
fn test_while_loop_carried_def() {
    let index = index(
        "\
x <- 0          # def 0
while (cond) {
    x           # use 1: sees {def 0, def 1} (pre-loop OR previous iteration)
    x <- 1      # def 1
}
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // Uses: `cond` is use 0, `x` inside body is use 1.
    let bindings = map.bindings_at_use(UseId::from(1));
    assert_eq!(bindings.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(1)
    ]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_for_loop_carried_def() {
    let index = index(
        "\
x <- 0              # def 0
for (i in xs) {     # def 1 (i)
    x               # use 1: sees {def 0, def 2}
    x <- 1          # def 2
}
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // Uses: `xs` is use 0, `x` inside body is use 1.
    let bindings = map.bindings_at_use(UseId::from(1));
    assert_eq!(bindings.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(2)
    ]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_repeat_loop_carried_def() {
    let index = index(
        "\
x <- 0       # def 0
repeat {
    x        # use 0: sees {def 0, def 1}
    x <- 1   # def 1
    break
}
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    let bindings = map.bindings_at_use(UseId::from(0));
    assert_eq!(bindings.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(1)
    ]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_loop_carried_unbound_before_loop() {
    let index = index(
        "\
while (cond) {
    x           # use 1: sees {def 0}, may_be_unbound
    x <- 1      # def 0
}
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // Uses: `cond` is use 0, `x` inside body is use 1.
    let bindings = map.bindings_at_use(UseId::from(1));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(0)]);
    assert!(bindings.may_be_unbound());
}

// --- Function scopes ---

#[test]
fn test_function_scope_independent_use_def() {
    let index = index(
        "\
x <- 1                  # file: def 0
f <- function(y) {      # file: def 1, fun: def 0 (y param)
    y                   # fun: use 0
}
",
    );
    let fun = ScopeId::from(1);
    let map = index.use_def_map(fun);

    // In function scope, `y` (use 0) sees the parameter (def 0).
    let bindings = map.bindings_at_use(UseId::from(0));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(0)]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_function_parameter_shadows() {
    let index = index(
        "\
function(x) {   # def 0 (param)
    x <- 1      # def 1
    x            # use 0 -> def 1
}
",
    );
    let fun = ScopeId::from(1);
    let map = index.use_def_map(fun);

    let bindings = map.bindings_at_use(UseId::from(0));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(1)]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_function_unbound_reference() {
    let index = index(
        "\
function() {
    x            # use 0: not bound in this scope
}
",
    );
    let fun = ScopeId::from(1);
    let map = index.use_def_map(fun);

    let bindings = map.bindings_at_use(UseId::from(0));
    assert!(bindings.definitions().is_empty());
    assert!(bindings.may_be_unbound());
}

// --- Super-assignment ---

#[test]
fn test_super_assignment_not_in_function_use_def() {
    let index = index(
        "\
function() {
    x <<- 1      # fun: def 0, recorded with IS_SUPER_BOUND, skipped by use-def
    x            # fun: use 0: unbound in function scope
}
",
    );
    let fun = ScopeId::from(1);
    let map = index.use_def_map(fun);

    let bindings = map.bindings_at_use(UseId::from(0));
    assert!(bindings.definitions().is_empty());
    assert!(bindings.may_be_unbound());
}

#[test]
fn test_super_assignment_visible_in_parent_use_def() {
    let index = index(
        "\
x <- 1                       # file: def 0
f <- function() { x <<- 2 }  # file: def 1 (x <<- extra def), def 2 (f)
x                            # file: use 0 -> {def 0, def 1}
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // The <<- extra definition (def 1) is recorded in the file scope
    // during function body processing, before the `f <-` assignment (def 2).
    let bindings = map.bindings_at_use(UseId::from(0));
    assert_eq!(bindings.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(1)
    ]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_super_assignment_without_prior_def() {
    let index = index(
        "\
f <- function() { x <<- 1 }  # file: def 0 (x <<- extra def), def 1 (f)
x                            # file: use 0 -> {def 0}, may_be_unbound
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    let bindings = map.bindings_at_use(UseId::from(0));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(0)]);
    assert!(bindings.may_be_unbound());
}

#[test]
fn test_super_assignment_visible_before_function_def() {
    let index = index(
        "\
x                            # file: use 0 -> {def 0}, may_be_unbound
f <- function() { x <<- 1 }  # file: def 0 (x <<- in parent), def 1 (f)
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // `<<-` definitions are scope-wide, so the use of `x` before the
    // function definition still sees the `<<-` binding.
    let bindings = map.bindings_at_use(UseId::from(0));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(0)]);
    assert!(bindings.may_be_unbound());
}

#[test]
fn test_super_assignment_merges_with_if() {
    let index = index(
        "\
x <- 1                            # file: def 0
if (cond) {
    f <- function() { x <<- 2 }   # file: def 1 (x <<- extra def), def 2 (f)
}
x                                 # use 1 -> {def 0, def 1}
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // Uses: `cond` is use 0, final `x` is use 1.
    let bindings = map.bindings_at_use(UseId::from(1));
    assert_eq!(bindings.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(1)
    ]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_super_assignment_targets_grandparent() {
    let index = index(
        "\
x <- 1                                       # file: def 0
f <- function() {                            # file: def 2 (f)
    g <- function() { x <<- 2 }              # file: def 1 (x <<-)
}
x                                            # file: use 0 -> {def 0, def 1}
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // `<<-` in g walks up: g's parent is f, f has no binding for x,
    // so it continues to file scope where x has IS_BOUND. The extra
    // binding lands in the file scope, skipping the intermediate f scope.
    let bindings = map.bindings_at_use(UseId::from(0));
    assert_eq!(bindings.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(1)
    ]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_super_assignment_targets_intermediate_scope() {
    let index = index(
        "\
x <- 1                                       # file: def 0
f <- function() {
    x <- 10                                  # outer: def 0
    g <- function() { x <<- 2 }              # outer: def 1 (x <<-)
}
x                                            # file: use 0 -> {def 0} only
",
    );
    let file = ScopeId::from(0);
    let outer = ScopeId::from(1);

    // `<<-` in g walks up: g's parent is f, f has x with IS_BOUND
    // (from `x <- 10`), so it targets f -- not the file scope.
    let file_map = index.use_def_map(file);
    let bindings = file_map.bindings_at_use(UseId::from(0));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(0)]);
    assert_not!(bindings.may_be_unbound());

    // The extra binding is in f's scope, not the file scope.
    let outer_defs: Vec<_> = index
        .definitions(outer)
        .iter()
        .filter(|(_, d)| index.symbols(outer).symbol_id(d.symbol()).name() == "x")
        .collect();
    assert_eq!(outer_defs.len(), 2);
}

// --- Combined control flow ---

#[test]
fn test_if_inside_for() {
    let index = index(
        "\
for (i in xs) { # def 0 (i)
    if (cond) {
        x <- 1  # def 1
    }
}
x               # use 2: may_be_unbound (loop might not run, if might not match)
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // Uses: `xs` is use 0, `cond` is use 1, final `x` is use 2.
    let bindings = map.bindings_at_use(UseId::from(2));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(1)]);
    assert!(bindings.may_be_unbound());
}

#[test]
fn test_if_else_inside_while() {
    let index = index(
        "\
x <- 0          # def 0
while (cond) {
    if (c2) {
        x <- 1  # def 1
    } else {
        x <- 2  # def 2
    }
}
x               # use 2 -> {def 0, def 1, def 2} (while may not execute)
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // Uses: `cond` is use 0, `c2` is use 1, final `x` is use 2.
    let bindings = map.bindings_at_use(UseId::from(2));
    assert_eq!(bindings.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(1),
        DefinitionId::from(2)
    ]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_assignment_in_if_condition() {
    // In R, `if (x <- f()) x` is valid: the `<-` in the condition creates
    // a binding. The use of `x` in the consequence should see it.
    let index = index("if (x <- f()) x");
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // Uses: `f` is use 0. The `x` in consequence is use 1.
    // Def 0 is `x <- f()`.
    let bindings = map.bindings_at_use(UseId::from(1));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(0)]);
    assert_not!(bindings.may_be_unbound());
}

// --- Duplicate definitions don't appear twice in merge ---

#[test]
fn test_merge_deduplicates() {
    let index = index(
        "\
x <- 1       # def 0
if (cond) {
    y <- 1   # def 1 (different symbol)
}
x            # use 1: should see only {def 0}, not {def 0, def 0}
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // Uses: `cond` is use 0, final `x` is use 1.
    let bindings = map.bindings_at_use(UseId::from(1));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(0)]);
    assert_not!(bindings.may_be_unbound());
}

// --- Control flow inside functions ---

#[test]
fn test_if_else_in_function() {
    let index = index(
        "\
function(x) {       # def 0 (param)
    if (x) {
        y <- 1      # def 1
    } else {
        y <- 2      # def 2
    }
    y               # use 1 -> {def 1, def 2}
}
",
    );
    let fun = ScopeId::from(1);
    let map = index.use_def_map(fun);

    // In function scope: uses are `x` (use 0), then `y` (use 1).
    let bindings = map.bindings_at_use(UseId::from(1));
    assert_eq!(bindings.definitions(), &[
        DefinitionId::from(1),
        DefinitionId::from(2)
    ]);
    assert_not!(bindings.may_be_unbound());
}

// --- Multiple unrelated symbols through control flow ---

#[test]
fn test_different_symbols_through_if() {
    let index = index(
        "\
x <- 1       # def 0
if (cond) {
    y <- 2   # def 1
}
x            # use 1: sees def 0, not unbound
y            # use 2: sees def 1, may_be_unbound
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    // Uses: `cond` is use 0, `x` is use 1, `y` is use 2.
    let x_bindings = map.bindings_at_use(UseId::from(1));
    assert_eq!(x_bindings.definitions(), &[DefinitionId::from(0)]);
    assert_not!(x_bindings.may_be_unbound());

    let y_bindings = map.bindings_at_use(UseId::from(2));
    assert_eq!(y_bindings.definitions(), &[DefinitionId::from(1)]);
    assert!(y_bindings.may_be_unbound());
}

// --- Connected component patterns from the design doc ---

#[test]
fn test_design_doc_disconnected_components() {
    let index = index(
        "\
x <- 1       # def 0
print(x)     # use 0 (print), use 1 (x) -> {def 0}
if (cond) {
    x <- 2   # def 1
} else {
    x <- 3   # def 2
}
print(x)     # use 2 (cond), use 3 (print), use 4 (x) -> {def 1, def 2}
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    let first_x = map.bindings_at_use(UseId::from(1));
    assert_eq!(first_x.definitions(), &[DefinitionId::from(0)]);
    assert_not!(first_x.may_be_unbound());

    let second_x = map.bindings_at_use(UseId::from(4));
    assert_eq!(second_x.definitions(), &[
        DefinitionId::from(1),
        DefinitionId::from(2)
    ]);
    assert_not!(second_x.may_be_unbound());
}

#[test]
fn test_design_doc_connected_component() {
    let index = index(
        "\
x <- 1       # def 0
print(x)     # use 0 (print), use 1 (x) -> {def 0}
if (cond) {
    x <- 2   # def 1
}
print(x)     # use 2 (cond), use 3 (print), use 4 (x) -> {def 0, def 1}
",
    );
    let file = ScopeId::from(0);
    let map = index.use_def_map(file);

    let first_x = map.bindings_at_use(UseId::from(1));
    assert_eq!(first_x.definitions(), &[DefinitionId::from(0)]);

    // Linked to both def 0 and def 1.
    let second_x = map.bindings_at_use(UseId::from(4));
    assert_eq!(second_x.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(1)
    ]);
    assert_not!(second_x.may_be_unbound());
}

// --- Cross-scope resolution ---

#[test]
fn test_cross_scope_simple_free_variable() {
    let index = index(
        "\
x <- 1
f <- function() x
",
    );
    let fun = ScopeId::from(1);

    // `x` in the function is free, resolves to file scope
    let (enclosing_scope, bindings) = index
        .enclosing_bindings(fun, index.uses(fun)[UseId::from(0)].symbol())
        .unwrap();
    assert_eq!(enclosing_scope, ScopeId::from(0));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(0)]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_cross_scope_def_after_function() {
    let index = index(
        "\
f <- function() x
x <- 1
",
    );
    let fun = ScopeId::from(1);

    // `x` is defined after `f` in the file scope. The pre-scan finds it.
    // The snapshot is initialized at f's definition point (x unbound)
    // then updated when x <- 1 is encountered.
    let (enclosing_scope, bindings) = index
        .enclosing_bindings(fun, index.uses(fun)[UseId::from(0)].symbol())
        .unwrap();
    assert_eq!(enclosing_scope, ScopeId::from(0));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(1)]);
    assert!(bindings.may_be_unbound());
}

#[test]
fn test_cross_scope_multiple_defs() {
    let index = index(
        "\
x <- 1
f <- function() x
x <- 2
",
    );
    let fun = ScopeId::from(1);

    // Lazy snapshot: union of all defs from definition point onward.
    // Initialized with {x <- 1}, updated with {x <- 2}.
    let (_, bindings) = index
        .enclosing_bindings(fun, index.uses(fun)[UseId::from(0)].symbol())
        .unwrap();
    assert_eq!(bindings.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(2)
    ]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_cross_scope_locally_bound_not_free() {
    let index = index(
        "\
x <- 1
f <- function() {
    x <- 2
    x
}
",
    );
    let fun = ScopeId::from(1);

    // `x` is locally bound in the function, not free
    assert!(index
        .enclosing_bindings(fun, index.uses(fun)[UseId::from(0)].symbol())
        .is_none());
}

#[test]
fn test_cross_scope_parameter_not_free() {
    let index = index(
        "\
x <- 1
f <- function(x) x
",
    );
    let fun = ScopeId::from(1);

    // `x` is a parameter, not free
    assert!(index
        .enclosing_bindings(fun, index.uses(fun)[UseId::from(0)].symbol())
        .is_none());
}

#[test]
fn test_cross_scope_nested_functions() {
    let index = index(
        "\
x <- 1
f <- function() {
    g <- function() x
}
",
    );
    // g is scope 2 (f is scope 1)
    let g_scope = ScopeId::from(2);

    // x is free in g. f (scope 1) has no binding for x, so the lookup
    // skips f entirely and resolves to the file scope (scope 0).
    let (enclosing_scope, bindings) = index
        .enclosing_bindings(g_scope, index.uses(g_scope)[UseId::from(0)].symbol())
        .unwrap();
    assert_eq!(enclosing_scope, ScopeId::from(0));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(0)]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_cross_scope_resolves_to_intermediate() {
    let index = index(
        "\
x <- 1
f <- function() {
    x <- 2
    g <- function() x
}
",
    );
    let g_scope = ScopeId::from(2);

    // x is free in g. Both the file scope (scope 0) and f (scope 1) bind x,
    // but f is the nearest enclosing scope with a binding, so it wins.
    let (enclosing_scope, bindings) = index
        .enclosing_bindings(g_scope, index.uses(g_scope)[UseId::from(0)].symbol())
        .unwrap();
    assert_eq!(enclosing_scope, ScopeId::from(1));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(0)]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_cross_scope_conditional_def_in_enclosing() {
    let index = index(
        "\
if (cond) x <- 1
f <- function() x
",
    );
    let fun = ScopeId::from(1);

    // x is conditionally defined. The snapshot captures the state at f's
    // definition point: {x <- 1, may_be_unbound: true}
    let (_, bindings) = index
        .enclosing_bindings(fun, index.uses(fun)[UseId::from(0)].symbol())
        .unwrap();
    assert_eq!(bindings.definitions(), &[DefinitionId::from(0)]);
    assert!(bindings.may_be_unbound());
}

#[test]
fn test_cross_scope_super_assignment_updates_snapshot() {
    let index = index(
        "\
x <- 1
f <- function() x
g <- function() { x <<- 2 }
",
    );
    let f_scope = ScopeId::from(1);

    // The <<- from g adds a def to the file scope. The watcher on x
    // should update f's snapshot to include this def.
    let (_, bindings) = index
        .enclosing_bindings(f_scope, index.uses(f_scope)[UseId::from(0)].symbol())
        .unwrap();
    assert_eq!(bindings.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(2)
    ]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_cross_scope_unbound_globally() {
    let index = index(
        "\
f <- function() x
",
    );
    let fun = ScopeId::from(1);

    // x is not defined anywhere in the file. No enclosing snapshot.
    assert!(index
        .enclosing_bindings(fun, index.uses(fun)[UseId::from(0)].symbol())
        .is_none());
}

#[test]
fn test_cross_scope_conditional_local_def_with_enclosing() {
    let index = index(
        "\
x <- 1
f <- function(cond) {
    if (cond) x <- 2
    x
}
",
    );
    let fun = ScopeId::from(1);
    let map = index.use_def_map(fun);

    // The use of `x` has a conditional local definition AND may_be_unbound.
    // In R, the unbound path falls through to the enclosing scope's x <- 1.
    // use 0 = `cond` (parameter reference in if condition)
    // use 1 = `x` (the use we're testing)
    let local = map.bindings_at_use(UseId::from(1));
    assert_eq!(local.definitions(), &[DefinitionId::from(1)]);
    assert!(local.may_be_unbound());

    // The enclosing snapshot should also be registered, capturing x <- 1.
    let (enclosing_scope, bindings) = index
        .enclosing_bindings(fun, index.uses(fun)[UseId::from(1)].symbol())
        .unwrap();
    assert_eq!(enclosing_scope, ScopeId::from(0));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(0)]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_cross_scope_conditional_local_def_in_loop() {
    let index = index(
        "\
x <- 1
f <- function() {
    for (i in 1:10) x <- 2
    x
}
",
    );
    let fun = ScopeId::from(1);
    let map = index.use_def_map(fun);

    // x is defined in the for body (conditional: body may not execute).
    // The use after the for loop has may_be_unbound: true.
    let local = map.bindings_at_use(UseId::from(0));
    assert!(local.may_be_unbound());

    // Enclosing snapshot registered for the fallthrough path.
    let (enclosing_scope, _) = index
        .enclosing_bindings(fun, index.uses(fun)[UseId::from(0)].symbol())
        .unwrap();
    assert_eq!(enclosing_scope, ScopeId::from(0));
}

#[test]
fn test_cross_scope_unconditional_local_def_no_snapshot() {
    let index = index(
        "\
x <- 1
f <- function() {
    x <- 2
    x
}
",
    );
    let fun = ScopeId::from(1);
    let map = index.use_def_map(fun);

    // x is unconditionally defined locally. No fallthrough possible.
    let local = map.bindings_at_use(UseId::from(0));
    assert_eq!(local.definitions(), &[DefinitionId::from(0)]);
    assert_not!(local.may_be_unbound());

    // No enclosing snapshot needed.
    assert!(index
        .enclosing_bindings(fun, index.uses(fun)[UseId::from(0)].symbol())
        .is_none());
}

#[test]
fn test_cross_scope_multiple_free_variables() {
    let index = index(
        "\
x <- 1
y <- 2
f <- function() {
    x
    y
}
",
    );
    let fun = ScopeId::from(1);

    // Two independent free variables, each gets its own snapshot
    let (scope_x, bindings_x) = index
        .enclosing_bindings(fun, index.uses(fun)[UseId::from(0)].symbol())
        .unwrap();
    assert_eq!(scope_x, ScopeId::from(0));
    assert_eq!(bindings_x.definitions(), &[DefinitionId::from(0)]);

    let (scope_y, bindings_y) = index
        .enclosing_bindings(fun, index.uses(fun)[UseId::from(1)].symbol())
        .unwrap();
    assert_eq!(scope_y, ScopeId::from(0));
    assert_eq!(bindings_y.definitions(), &[DefinitionId::from(1)]);
}

#[test]
fn test_cross_scope_same_free_var_used_twice() {
    let index = index(
        "\
x <- 1
f <- function() {
    x
    x
}
",
    );
    let fun = ScopeId::from(1);

    // Both uses of `x` are free and resolve to the same enclosing snapshot
    let (scope1, bindings1) = index
        .enclosing_bindings(fun, index.uses(fun)[UseId::from(0)].symbol())
        .unwrap();
    let (scope2, bindings2) = index
        .enclosing_bindings(fun, index.uses(fun)[UseId::from(1)].symbol())
        .unwrap();
    assert_eq!(scope1, scope2);
    assert_eq!(bindings1, bindings2);
}

#[test]
fn test_cross_scope_free_var_in_function_inside_loop() {
    let index = index(
        "\
x <- 1
for (i in 1:10) {
    f <- function() x
}
x <- 2
",
    );
    // The function scope: for doesn't create a scope, so f's function
    // is the only child scope.
    let fun = ScopeId::from(1);

    // x is free in f, resolves to file scope. The lazy snapshot
    // captures both x <- 1 (from initialization) and x <- 2 (from
    // watcher update).
    let (enclosing_scope, bindings) = index
        .enclosing_bindings(fun, index.uses(fun)[UseId::from(0)].symbol())
        .unwrap();
    assert_eq!(enclosing_scope, ScopeId::from(0));
    assert_eq!(bindings.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(3)
    ]);
}

#[test]
fn test_cross_scope_repeated_use_reuses_snapshot() {
    let index = index(
        "\
if (cond) x <- 1
f <- function() {
    x
    x
}
",
    );
    let fun = ScopeId::from(1);
    let map = index.use_def_map(fun);

    // Both uses of `x` are free and resolve to the same enclosing
    // snapshot. The first use triggers registration, the second reuses
    // the existing entry (dedup via EnclosingSnapshotKey).
    let local0 = map.bindings_at_use(UseId::from(0));
    assert!(local0.definitions().is_empty());
    assert!(local0.may_be_unbound());

    let (scope0, bindings0) = index
        .enclosing_bindings(fun, index.uses(fun)[UseId::from(0)].symbol())
        .unwrap();
    let (scope1, bindings1) = index
        .enclosing_bindings(fun, index.uses(fun)[UseId::from(1)].symbol())
        .unwrap();
    assert_eq!(scope0, scope1);
    assert_eq!(bindings0, bindings1);
    assert_eq!(bindings0.definitions(), &[DefinitionId::from(0)]);
    assert!(bindings0.may_be_unbound());
}

#[test]
fn test_cross_scope_nested_conditional_fallthrough() {
    let index = index(
        "\
x <- 1
f <- function(cond) {
    if (cond) x <- 2
    g <- function() x
}
",
    );
    // g is scope 2 (f is scope 1)
    let g_scope = ScopeId::from(2);

    // x is free in g. Both the file scope (scope 0, unconditional x <- 1) and
    // f (scope 1, conditional x <- 2) bind x. f is the nearest enclosing
    // scope with a binding, so it wins. The snapshot captures f's state at
    // g's definition point: {x <- 2, may_be_unbound: true}.
    let (enclosing_scope, bindings) = index
        .enclosing_bindings(g_scope, index.uses(g_scope)[UseId::from(0)].symbol())
        .unwrap();
    assert_eq!(enclosing_scope, ScopeId::from(1));
    assert_eq!(bindings.definitions(), &[DefinitionId::from(1)]);
    assert!(bindings.may_be_unbound());
}

#[test]
fn test_cross_scope_snapshot_excludes_shadowed_defs() {
    let index = index(
        "\
x <- 0
x <- 1
f <- function() x
",
    );
    let fun = ScopeId::from(1);

    // x <- 0 was shadowed by x <- 1 before f was defined.
    // The snapshot should contain only x <- 1, not both.
    let (_, bindings) = index
        .enclosing_bindings(fun, index.uses(fun)[UseId::from(0)].symbol())
        .unwrap();
    assert_eq!(bindings.definitions(), &[DefinitionId::from(1)]);
    assert_not!(bindings.may_be_unbound());
}

#[test]
fn test_cross_scope_different_definition_points() {
    let index = index(
        "\
x <- 1
f <- function() x
x <- 2
g <- function() x
",
    );

    // f is defined after x <- 1. Its snapshot is initialized with {x <- 1},
    // then the watcher adds x <- 2: snapshot {x <- 1, x <- 2}.
    let f_scope = ScopeId::from(1);
    let (_, f_bindings) = index
        .enclosing_bindings(f_scope, index.uses(f_scope)[UseId::from(0)].symbol())
        .unwrap();
    assert_eq!(f_bindings.definitions(), &[
        DefinitionId::from(0),
        DefinitionId::from(2)
    ]);
    assert_not!(f_bindings.may_be_unbound());

    // g is defined after x <- 2, which shadowed x <- 1. Its snapshot is
    // initialized with {x <- 2} only. No subsequent definitions, so it
    // stays {x <- 2}.
    let g_scope = ScopeId::from(2);
    let (_, g_bindings) = index
        .enclosing_bindings(g_scope, index.uses(g_scope)[UseId::from(0)].symbol())
        .unwrap();
    assert_eq!(g_bindings.definitions(), &[DefinitionId::from(2)]);
    assert_not!(g_bindings.may_be_unbound());
}
