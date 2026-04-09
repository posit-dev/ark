use aether_parser::parse;
use aether_parser::RParserOptions;
use oak_index::builder::build;
use oak_index::semantic_index::DefinitionId;
use oak_index::semantic_index::ScopeId;
use oak_index::semantic_index::SemanticIndex;
use oak_index::semantic_index::UseId;
use stdext::assert_not;

fn index(source: &str) -> SemanticIndex {
    let parsed = parse(source, RParserOptions::default());
    build(&parsed.tree())
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
    x <<- 1      # recorded here with IS_SUPER_BOUND, skipped by use-def
    x            # use 0: unbound in function scope
}
",
    );
    let fun = ScopeId::from(1);
    let map = index.use_def_map(fun);

    let bindings = map.bindings_at_use(UseId::from(0));
    assert!(bindings.definitions().is_empty());
    assert!(bindings.may_be_unbound());
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
    assert!(bindings.definitions().contains(&DefinitionId::from(1)));
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
