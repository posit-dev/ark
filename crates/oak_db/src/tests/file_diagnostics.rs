//! Snapshot tests for diagnostics rendering. Each test is one case, with a
//! comment explaining whether it's correct-by-design or a known gap.

use crate::tests::diagnostic_render::render;
use crate::tests::resolver::install_packages;
use crate::tests::test_db::file_path;
use crate::tests::test_db::TestDb;
use crate::File;
use crate::FileRevision;

fn new_file(db: &TestDb, name: &str, contents: &str) -> File {
    File::new(
        db,
        file_path(name),
        FileRevision::zero(),
        Some(contents.to_string()),
        None,
    )
}

#[test]
fn test_diagnostic_ambiguous_attach_order() {
    // The arms attach the same two packages in opposite orders, so which one
    // masks the other after the `if` depends on the branch taken. Flagged
    // even though `cli` and `rlang` share no names today.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli", "rlang"]);
    let source = "\
if (cond) {
    library(cli)
    library(rlang)
} else {
    library(rlang)
    library(cli)
}
";
    let file = new_file(&db, "a.R", source);

    insta::assert_snapshot!(render("a.R", source, file.diagnostics(&db)));
}

#[test]
fn test_diagnostic_argument_matching_no_scope_silent() {
    // Correct by design, silent. `test_that(1)` has no second argument at
    // all, so there's no `code` block to scope on either path. The
    // conditional `test_that <- identity` has nothing to shadow.
    let mut db = TestDb::new();
    install_packages(&mut db, &["testthat"]);
    let source = "\
library(testthat)
if (cond) {
    test_that <- identity
}
test_that(1)
";
    let file = new_file(&db, "a.R", source);

    insta::assert_snapshot!(render("a.R", source, file.diagnostics(&db)));
}

#[test]
fn test_diagnostic_conditional_attach_branch_join() {
    // `library(shiny)` attaches on only the `if` path, so the attach drops at
    // the join and `reactive()` reads as plain. `reactive`'s NSE annotation
    // comes from the static effect registry keyed on the package name
    // `shiny`, which needs `shiny` to resolve as an installed package.
    // Unlike base functions, non-base packages have no hardcoded fallback.
    let mut db = TestDb::new();
    install_packages(&mut db, &["shiny"]);
    let source = "\
if (cond) library(shiny)
reactive({
    x <- 1
})
";
    let file = new_file(&db, "a.R", source);

    insta::assert_snapshot!(render("a.R", source, file.diagnostics(&db)));
}

#[test]
fn test_diagnostic_conditional_attach_wins_over_shadow() {
    // The `if` branch attaches shiny and the `else` branch conditionally
    // shadows `reactive`, so both branches create a join-scope ambiguity.
    // Only the attach diagnostic fires for this call. The shadow diagnostic
    // doesn't also pile on.
    let mut db = TestDb::new();
    install_packages(&mut db, &["shiny"]);
    let source = "\
if (sample(0:1, 1)) {
    library(shiny)
} else {
    reactive <- identity
}
reactive(1)
";
    let file = new_file(&db, "a.R", source);

    insta::assert_snapshot!(render("a.R", source, file.diagnostics(&db)));
}

#[test]
fn test_diagnostic_conditional_shadow_definite_silent() {
    // Correct by design, silent. `local` is reassigned before the call on
    // the only path there is, so by the time `local({ x <- 1 })` runs,
    // `local` is plain `identity()`. There's no NSE effect left to be
    // ambiguous about.
    let db = TestDb::new();
    let source = "\
local <- identity
local({ x <- 1 })
";
    let file = new_file(&db, "a.R", source);

    insta::assert_snapshot!(render("a.R", source, file.diagnostics(&db)));
}

#[test]
fn test_diagnostic_conditional_shadow_eager() {
    // The inner `local({ y <- 1 })` is flagged because `local` is
    // conditionally reassigned earlier in the same outer scope, which is an
    // eager NSE scope. That conditional binding could shadow the inner
    // call, so it's ambiguous. This uses base `local()`, so no package
    // fixture is needed.
    let db = TestDb::new();
    let source = "\
local({
    if (cond) local <- identity
    local({
        y <- 1
    })
})
";
    let file = new_file(&db, "a.R", source);

    insta::assert_snapshot!(render("a.R", source, file.diagnostics(&db)));
}

#[test]
fn test_diagnostic_conditional_shadow_package_call() {
    // A conditional reassignment of `test_that` earlier in the same eager
    // scope makes the later `test_that(...)` call ambiguous, the same shape
    // as the base `local()` case, but through a real package effect
    // (`testthat::test_that()`) instead of the hardcoded base fallback.
    let mut db = TestDb::new();
    install_packages(&mut db, &["testthat"]);
    let source = "\
library(testthat)
if (cond) {
    test_that <- identity
}
test_that(\"d\", { x <- 1 })
";
    let file = new_file(&db, "a.R", source);

    insta::assert_snapshot!(render("a.R", source, file.diagnostics(&db)));
}

#[test]
fn test_diagnostic_gap_conditional_shadow_enclosing_scope() {
    // Known gap, silent. `local` is conditionally shadowed at file scope,
    // and the inner `local({ y <- 1 })` runs inside `with()`'s eager nested
    // scope, so the shadow really can reach it. But
    // `record_conditional_shadow_ambiguity()` in
    // `crates/oak_semantic/src/builder/scan.rs` only checks the current
    // scan scope, which is `with()`'s body, not the file scope where the
    // conditional assignment actually lives, so it misses this case.
    let db = TestDb::new();
    let source = "\
if (cond) local <- identity
with(d, {
    local({ y <- 1 })
})
";
    let file = new_file(&db, "a.R", source);

    insta::assert_snapshot!(render("a.R", source, file.diagnostics(&db)));
}

#[test]
fn test_diagnostic_gap_lazy_sibling_attach() {
    // Known gap, silent. `g`'s `library(shiny)` never runs, since nothing
    // calls `g`. Even if it did, `record_conditional_attach_ambiguity()`'s
    // call-site probe only sees attaches reachable from its own scan, so it
    // can't tell that `f`'s `reactive()` might one day run after `g`.
    // Catching this needs a whole-file post-pass over lazy contexts, not a
    // call-site probe (see the doc comment on
    // `record_conditional_attach_ambiguity()` in
    // `crates/oak_semantic/src/builder/effects.rs`).
    let mut db = TestDb::new();
    install_packages(&mut db, &["shiny"]);
    let source = "\
g <- function() library(shiny)
f <- function() reactive({ x <- 1 })
";
    let file = new_file(&db, "a.R", source);

    insta::assert_snapshot!(render("a.R", source, file.diagnostics(&db)));
}

#[test]
fn test_diagnostic_gap_named_arg_before_block() {
    // R matches named arguments first, so `desc = "d"` binds to the `desc`
    // formal, and the unnamed block then fills the remaining `code` formal,
    // which is formal position 1, even though the block sits at call
    // position 0. `match_positional()` in `crates/oak_semantic/src/effects.rs`
    // only matches a positional argument to a formal declared at that exact
    // call position, so it never finds `code` here and no scope gets pushed
    // for `x <- 1`. Confirmed by direct comparison: this source yields one
    // scope, versus two for the same call with `code` in its normal
    // position, so `x` resolves at file scope instead of inside
    // `test_that()`.
    let mut db = TestDb::new();
    install_packages(&mut db, &["testthat"]);
    let source = "\
library(testthat)
if (cond) {
    test_that <- identity
}
test_that({ x <- 1 }, desc = \"d\")
";
    let file = new_file(&db, "a.R", source);

    insta::assert_snapshot!(render("a.R", source, file.diagnostics(&db)));
}

#[test]
fn test_diagnostic_installed_package_silent() {
    // Correct by design, silent. `shiny` is registered as installed, so the
    // attach resolves and there's nothing to report about it.
    let mut db = TestDb::new();
    install_packages(&mut db, &["shiny"]);
    let source = "library(shiny)\n";
    let file = new_file(&db, "a.R", source);

    insta::assert_snapshot!(render("a.R", source, file.diagnostics(&db)));
}

#[test]
fn test_diagnostic_lazy_shadow_interleaved() {
    // `f`'s body calls both `with()` and a nested `local()`, and both
    // symbols are reassigned afterwards at file scope, so each call is
    // flagged for the same lazy-timing reason: an assignment at file scope
    // that might run before the call. The two calls share a line, so this
    // also checks marker ordering: the outer `with()` call starts before
    // the nested `local()` call, and both marker rows must appear in that
    // order.
    let db = TestDb::new();
    let source = "\
f <- function() with(local({ x <- 1 }), { y <- 2 })
local <- identity
with <- identity
";
    let file = new_file(&db, "a.R", source);

    insta::assert_snapshot!(render("a.R", source, file.diagnostics(&db)));
}

#[test]
fn test_diagnostic_lazy_shadow_reassignment() {
    // `f` is never called, so there's no way to know whether
    // `local <- identity` at file scope would run before or after `f`'s
    // eventual call. That undetermined timing is why the inner
    // `local({ x <- 1 })` is flagged. This exercises base's `local()` NSE
    // annotation, which resolves without any package fixture:
    // `SalsaImportsResolver` falls back to a static base registry when
    // nothing else applies.
    let db = TestDb::new();
    let source = "\
f <- function() local({ x <- 1 })
local <- identity
";
    let file = new_file(&db, "a.R", source);

    insta::assert_snapshot!(render("a.R", source, file.diagnostics(&db)));
}

#[test]
fn test_diagnostic_unconditional_attach_silent() {
    // Correct by design, silent. `library(shiny)` attaches on every path,
    // so `reactive()` is unambiguously NSE. Nothing competes with it.
    let mut db = TestDb::new();
    install_packages(&mut db, &["shiny"]);
    let source = "\
library(shiny)
reactive({ x <- 1 })
";
    let file = new_file(&db, "a.R", source);

    insta::assert_snapshot!(render("a.R", source, file.diagnostics(&db)));
}

#[test]
fn test_diagnostic_uninstalled_package_conditional() {
    // Same source as the conditional-attach-at-a-branch-join case, but this
    // time `shiny` is never registered as an installed package, deliberately.
    // Without a resolvable package, `reactive` never gets an NSE annotation in
    // the first place, so there's no attach to be conditional about, and the
    // ambiguity diagnostic can't fire. We report the uninstalled package
    // instead, so the user still gets a signal that analysis is degraded here.
    let db = TestDb::new();
    let source = "\
if (cond) library(shiny)
reactive({
    x <- 1
})
";
    let file = new_file(&db, "a.R", source);

    insta::assert_snapshot!(render("a.R", source, file.diagnostics(&db)));
}

#[test]
fn test_diagnostic_uninstalled_package_unconditional() {
    let db = TestDb::new();
    let source = "library(shiny)\n";
    let file = new_file(&db, "a.R", source);

    insta::assert_snapshot!(render("a.R", source, file.diagnostics(&db)));
}
