//! Snapshot tests for diagnostics rendering. Each test is one case, with a
//! comment explaining whether it's correct-by-design or a known gap.

use oak_package_metadata::namespace::Import;
use oak_package_metadata::namespace::Namespace;
use salsa::Setter;
use stdext::SortedVec;

use crate::tests::diagnostic_render::render;
use crate::tests::resolver::install_packages;
use crate::tests::test_db::file_path;
use crate::tests::test_db::library_root;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::DbInputs;
use crate::File;
use crate::FileRevision;
use crate::Package;

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
fn test_diagnostic_named_arg_before_block_scopes_correctly() {
    // After `desc` binds by name, the block fills `code` despite appearing first.
    // Its `test_that()` resolution is conditionally shadowed and must still gain
    // the `code` scope.
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

#[test]
fn test_diagnostic_source_cycle() {
    // `a.R` and `b.R` source each other, which R can't run. The recovery that
    // breaks the salsa cycle is the only place that knows, so the diagnostic is
    // raised there and anchored at the start of the file: under
    // `NoopImportsResolver` a bare `source()` isn't recognized as effectful, so
    // there's no recorded call to point at.
    let mut db = TestDb::new();
    let a_source = "source(\"b.R\")\n";
    let (a, _b) = cyclic_pair(&mut db, a_source, "source(\"a.R\")\n");

    insta::assert_snapshot!(render("w/a.R", a_source, a.diagnostics(&db)));
}

#[test]
fn test_diagnostic_source_cycle_reported_on_both_files() {
    // Salsa's `FallbackImmediate` hands every participant its fallback, not just
    // the one it re-entered, so both files are rebuilt degraded and both warn.
    // That's what lets a file-local diagnostic cover a cycle it can't name:
    // whichever file the user has open carries its own copy.
    let mut db = TestDb::new();
    let b_source = "source(\"a.R\")\n";
    let (a, b) = cyclic_pair(&mut db, "source(\"b.R\")\n", b_source);

    assert_eq!(a.diagnostics(&db).len(), 1);
    insta::assert_snapshot!(render("w/b.R", b_source, b.diagnostics(&db)));
}

#[test]
fn test_diagnostic_inherited_shadow() {
    // `a.R` binds `source` and then sources `b.R` through `base::source`, which
    // no binding can shadow. Inside `b.R`, its own bare `source("c.R")` was
    // analysed as base `source` (the scan knows nothing about source sites), but
    // `b.R`'s `imports()` now reaches `a.R`'s `source <- identity` through the
    // inherited band. That's the one call the two disagree about, so we say so
    // instead of silently picking a side.
    let mut db = TestDb::new();
    let root = workspace_root(&db, "w");
    let a = new_file(&db, "w/a.R", "source <- identity\nbase::source(\"b.R\")\n");
    let b_source = "foo <- 1\nsource(\"c.R\")\n";
    let b = new_file(&db, "w/b.R", b_source);
    let c = new_file(&db, "w/c.R", "foo\n");
    root.set_scripts(&mut db).to(vec![a, b, c]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    insta::assert_snapshot!(render("w/b.R", b_source, b.diagnostics(&db)));
}

#[test]
fn test_diagnostic_no_inherited_shadow_for_ordinary_sourcing() {
    // The check must not fire just because the inherited search path reaches
    // `base`, which really does bind `source`. Both views settle on the same
    // `Package(base)` layer, so there's no disagreement to report.
    //
    // `install_package_binding` rather than `install_packages`: the latter
    // registers a package with no files, so no `Package` layer binds anything
    // and the case this test is about wouldn't arise at all.
    let mut db = TestDb::new();
    install_package_binding(&mut db, "base", &["source"]);
    let root = workspace_root(&db, "w");
    let main = new_file(&db, "w/main.R", "source(\"helpers.R\")\n");
    let helpers_source = "source(\"more.R\")\n";
    let helpers = new_file(&db, "w/helpers.R", helpers_source);
    let more = new_file(&db, "w/more.R", "x <- 1\n");
    root.set_scripts(&mut db).to(vec![main, helpers, more]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    insta::assert_snapshot!(render(
        "w/helpers.R",
        helpers_source,
        helpers.diagnostics(&db)
    ));
}

#[test]
fn test_diagnostic_no_inherited_shadow_when_file_binds_the_name_itself() {
    // `helpers.R` binds `source` itself, after the call, so the scan does treat
    // the call as effectful. But `imports()` resolves `source` to that own
    // binding rather than to the inherited one, so the inherited layer isn't what
    // makes this call uncertain. Authored shadowing is `EffectAmbiguity`'s job.
    let mut db = TestDb::new();
    let root = workspace_root(&db, "w");
    let main = new_file(
        &db,
        "w/main.R",
        "source <- identity\nbase::source(\"helpers.R\")\n",
    );
    let helpers_source = "source(\"more.R\")\nsource <- identity\n";
    let helpers = new_file(&db, "w/helpers.R", helpers_source);
    let more = new_file(&db, "w/more.R", "x <- 1\n");
    root.set_scripts(&mut db).to(vec![main, helpers, more]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    insta::assert_snapshot!(render(
        "w/helpers.R",
        helpers_source,
        helpers.diagnostics(&db)
    ));
}

#[test]
fn test_diagnostic_inherited_attach_shadows_a_callee() {
    // `main.R` attaches a package exporting its own `library`, and that attach
    // reaches `helpers.R` only through inheritance. So `helpers.R`'s scan
    // resolved its `library(dplyr)` callee to base `library`, while `imports()`
    // resolves it to `shadowr::library`. This is the case a check keyed on `File`
    // layers alone misses, and attach ordering is most of what inheritance
    // contributes.
    //
    // `shadowr` deliberately shadows `library` rather than `source`: shadowing
    // `source` would also shadow `main.R`'s own `source("helpers.R")` call, which
    // would stop being effectful and take the whole inheritance edge with it.
    let mut db = TestDb::new();
    install_package_binding(&mut db, "base", &["source", "library"]);
    install_package_binding(&mut db, "shadowr", &["library"]);
    install_package_binding(&mut db, "dplyr", &[]);
    let root = workspace_root(&db, "w");
    let main = new_file(&db, "w/main.R", "library(shadowr)\nsource(\"helpers.R\")\n");
    let helpers_source = "library(dplyr)\n";
    let helpers = new_file(&db, "w/helpers.R", helpers_source);
    root.set_scripts(&mut db).to(vec![main, helpers]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    insta::assert_snapshot!(render(
        "w/helpers.R",
        helpers_source,
        helpers.diagnostics(&db)
    ));
}

#[test]
fn test_diagnostic_no_inherited_shadow_when_neither_binding_is_effectful() {
    // `zzz-shadow.R` and `main.R` both bind `library` to a plain function, so
    // either winner leaves the call equally non-NSE.
    //
    // The shadow must be a successor. A predecessor prevents `library(dplyr)`
    // from reaching `semantic_calls()` during the eager scan.
    let mut db = TestDb::new();
    install_package_binding(&mut db, "base", &["source", "library"]);
    install_package_binding(&mut db, "dplyr", &[]);
    let root = workspace_root(&db, "w");
    let main = new_file(
        &db,
        "w/main.R",
        "library <- identity\nsource(\"R/helpers.R\")\n",
    );
    let helpers_source = "library(dplyr)\n";
    let helpers = new_file(&db, "w/R/helpers.R", helpers_source);
    let shadow = new_file(&db, "w/R/zzz-shadow.R", "library <- function(...) NULL\n");
    root.set_scripts(&mut db).to(vec![main, helpers, shadow]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    insta::assert_snapshot!(render(
        "w/R/helpers.R",
        helpers_source,
        helpers.diagnostics(&db)
    ));
}

#[test]
fn test_diagnostic_inherited_namespace_import_shadows_a_callee() {
    // Same shape as `test_diagnostic_inherited_attach_shadows_a_callee`, but the
    // sourcing side reaches `library` through `mypkg`'s NAMESPACE
    // (`importFrom(shadowr, library)`) rather than through a `library()` call,
    // pinning the `ImportLayer::From` arm of `describe_source`.
    //
    // `mypkg` imports `library` rather than `source` deliberately. Shadowing
    // `source` would stop `main.R`'s own `source("helpers.R")` from being
    // effectful and take the inheritance edge with it.
    let mut db = TestDb::new();
    install_package_binding(&mut db, "base", &["source", "library"]);
    install_package_binding(&mut db, "shadowr", &["library"]);
    install_package_binding(&mut db, "dplyr", &[]);

    let root = workspace_root(&db, "w/mypkg");
    let namespace = Namespace {
        imports: vec![Import {
            name: "library".to_string(),
            package: "shadowr".to_string(),
        }],
        ..Default::default()
    };
    let pkg = Package::new(
        &db,
        file_path("w/mypkg/DESCRIPTION"),
        "mypkg".to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        Some(namespace),
        Vec::new(),
        Vec::new(),
    );
    let main = File::new(
        &db,
        file_path("w/mypkg/R/main.R"),
        FileRevision::zero(),
        Some("source(\"helpers.R\")\n".to_string()),
        Some(pkg),
    );
    pkg.set_files(&mut db).to(vec![main]);
    root.set_packages(&mut db).to(vec![pkg]);

    let helpers_source = "library(dplyr)\n";
    let helpers = new_file(&db, "w/mypkg/helpers.R", helpers_source);
    root.set_scripts(&mut db).to(vec![helpers]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    // Fails fast if `source()` path anchoring changes, since the whole fixture
    // rests on this edge existing.
    assert_eq!(helpers.sourced_by(&db).as_slice(), [main]);

    insta::assert_snapshot!(render(
        "w/mypkg/helpers.R",
        helpers_source,
        helpers.diagnostics(&db)
    ));
}

#[test]
fn test_diagnostic_no_inherited_shadow_for_own_attach_ordering() {
    // `helpers.R`'s `library(dplyr)` runs inside `local()`, before its own
    // top-level `library(shadowr)`, so the scan resolved that callee to base
    // `library` while the end-of-file read view resolves it to
    // `shadowr::library`. A real disagreement, but caused by `helpers.R`'s own
    // ordering, not by anything it inherits, so blaming the source site would be
    // wrong. Authored shadowing belongs to `EffectAmbiguity`.
    let mut db = TestDb::new();
    install_package_binding(&mut db, "base", &["source", "library"]);
    install_package_binding(&mut db, "shadowr", &["library"]);
    install_package_binding(&mut db, "dplyr", &[]);
    let root = workspace_root(&db, "w");
    let main = new_file(&db, "w/main.R", "source(\"helpers.R\")\n");
    let helpers_source = "local({\n  library(dplyr)\n})\nlibrary(shadowr)\n";
    let helpers = new_file(&db, "w/helpers.R", helpers_source);
    root.set_scripts(&mut db).to(vec![main, helpers]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    insta::assert_snapshot!(render(
        "w/helpers.R",
        helpers_source,
        helpers.diagnostics(&db)
    ));
}

/// Register an installed package that really binds and exports `symbols`, so
/// `Package::resolve` finds them. `install_packages` registers packages with no
/// files, which makes every `Package` layer inert. Additive across calls, so
/// several fixtures can coexist.
fn install_package_binding(db: &mut TestDb, name: &str, symbols: &[&str]) {
    let root = library_root(db, &format!("libs/{name}"));
    let namespace = Namespace {
        exports: SortedVec::from_vec(symbols.iter().map(|s| s.to_string()).collect()),
        ..Default::default()
    };
    let pkg = Package::new(
        db,
        file_path(&format!("libs/{name}/DESCRIPTION")),
        name.to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        Some(namespace),
        Vec::new(),
        Vec::new(),
    );
    let contents: String = symbols
        .iter()
        .map(|symbol| format!("{symbol} <- function(...) NULL\n"))
        .collect();
    let file = File::new(
        db,
        file_path(&format!("libs/{name}/R/exports.R")),
        FileRevision::zero(),
        Some(contents),
        Some(pkg),
    );
    pkg.set_files(db).to(vec![file]);
    root.set_packages(db).to(vec![pkg]);

    let mut roots = db.library_roots().roots(db).clone();
    roots.push(root);
    db.library_roots().set_roots(db).to(roots);
}

/// Two workspace scripts at `w/a.R` and `w/b.R`, whose contents are expected to
/// `source()` each other.
fn cyclic_pair(db: &mut TestDb, a_source: &str, b_source: &str) -> (File, File) {
    let root = workspace_root(db, "w");
    let a = new_file(db, "w/a.R", a_source);
    let b = new_file(db, "w/b.R", b_source);
    root.set_scripts(db).to(vec![a, b]);
    db.workspace_roots().set_roots(db).to(vec![root]);
    (a, b)
}
