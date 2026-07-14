use std::collections::HashSet;

use oak_package_metadata::namespace::Import;
use oak_package_metadata::namespace::Namespace;
use oak_semantic::semantic_index::DefinitionKind;
use oak_semantic::semantic_index::NseScope;
use oak_semantic::semantic_index::NseTiming;
use oak_semantic::semantic_index::ScopeId;
use oak_semantic::semantic_index::ScopeKind;
use oak_semantic::semantic_index::SemanticCallKind;
use salsa::Setter;
use stdext::SortedVec;

use crate::tests::test_db::file_path;
use crate::tests::test_db::library_root;
use crate::tests::test_db::make_package;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::DbInputs;
use crate::File;
use crate::FileRevision;
use crate::Package;
use crate::Root;

fn make_script(db: &mut TestDb, name: &str, contents: &str) -> File {
    File::new(
        db,
        file_path(name),
        FileRevision::zero(),
        Some(contents.to_string()),
        None,
    )
}

/// Build a fresh workspace root, attach the given scripts, register
/// it on the singleton `WorkspaceRoots` input.
fn setup_workspace(db: &mut TestDb, scripts: &[(&str, &str)]) -> (Root, Vec<File>) {
    let root = workspace_root(db, "");
    let scripts: Vec<File> = scripts
        .iter()
        .map(|(name, contents)| make_script(db, name, contents))
        .collect();
    root.set_scripts(db).to(scripts.clone());
    db.workspace_roots().set_roots(db).to(vec![root]);
    (root, scripts)
}

/// Build a `pkg`-named workspace package with `files` as its collation-ordered
/// `R/` sources and an empty NAMESPACE. Returns the files in collation order.
fn setup_package(db: &mut TestDb, files: &[(&str, &str)]) -> Vec<File> {
    let root = workspace_root(db, "pkg");
    let pkg = Package::new(
        db,
        file_path("pkg/DESCRIPTION"),
        "pkg".to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        Some(Namespace::default()),
        Vec::new(),
        Vec::new(),
    );
    let entities: Vec<File> = files
        .iter()
        .map(|(path, contents)| {
            File::new(
                db,
                file_path(path),
                FileRevision::zero(),
                Some(contents.to_string()),
                Some(pkg),
            )
        })
        .collect();
    pkg.set_files(db).to(entities.clone());
    root.set_packages(db).to(vec![pkg]);
    db.workspace_roots().set_roots(db).to(vec![root]);
    entities
}

/// Build a `pkg` package whose `tests/testthat/` files are `scripts` (so they
/// resolve as test files, not `R/` sources). Returns the script files in the
/// given order.
fn setup_testthat(db: &mut TestDb, scripts: &[(&str, &str)]) -> Vec<File> {
    let root = workspace_root(db, "pkg");
    let pkg = Package::new(
        db,
        file_path("pkg/DESCRIPTION"),
        "pkg".to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        Some(Namespace::default()),
        Vec::new(),
        Vec::new(),
    );
    let entities: Vec<File> = scripts
        .iter()
        .map(|(path, contents)| {
            File::new(
                db,
                file_path(path),
                FileRevision::zero(),
                Some(contents.to_string()),
                Some(pkg),
            )
        })
        .collect();
    pkg.set_scripts(db).to(entities.clone());
    root.set_packages(db).to(vec![pkg]);
    db.workspace_roots().set_roots(db).to(vec![root]);
    entities
}

/// Register bare installed packages (empty namespace) on `LibraryRoots`, one
/// root each, so `package_by_name` finds them. Their NSE effects come from the
/// static registry keyed on the name, so no namespace is needed here.
fn install_packages(db: &mut TestDb, names: &[&str]) {
    let roots: Vec<Root> = names
        .iter()
        .map(|&name| {
            let root = library_root(db, &format!("libs/{name}"));
            let pkg = Package::new(
                db,
                file_path(&format!("libs/{name}/DESCRIPTION")),
                name.to_string(),
                FileRevision::zero(),
                FileRevision::zero(),
                None,
                None,
                Vec::new(),
                Vec::new(),
            );
            root.set_packages(db).to(vec![pkg]);
            root
        })
        .collect();
    db.library_roots().set_roots(db).to(roots);
}

#[test]
fn test_testthat_test_that_is_nse_without_library() {
    // The runner attaches testthat before a test file's body runs, so a bare
    // `test_that` resolves to its NSE annotation with no `library(testthat)`.
    let mut db = TestDb::new();
    install_packages(&mut db, &["testthat"]);
    let files = setup_testthat(&mut db, &[(
        "pkg/tests/testthat/test-a.R",
        "test_that(\"x\", {\n    y <- 1\n})\n",
    )]);

    let index = files[0].semantic_index(&db);
    let test_scope = ScopeId::from(1);
    assert_eq!(index.scope_ids().count(), 2);
    assert_eq!(
        index.scope(test_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
}

#[test]
fn test_testthat_helper_attach_enables_nse() {
    // A `helper*.R` file is sourced into the test env before the body runs, so
    // its `library(shiny)` puts shiny on the search path and the test's bare
    // `reactive` is NSE.
    let mut db = TestDb::new();
    install_packages(&mut db, &["shiny"]);
    let files = setup_testthat(&mut db, &[
        ("pkg/tests/testthat/helper-x.R", "library(shiny)\n"),
        (
            "pkg/tests/testthat/test-a.R",
            "reactive({\n    x <- 1\n})\n",
        ),
    ]);

    let index = files[1].semantic_index(&db);
    let reactive_scope = ScopeId::from(1);
    assert_eq!(index.scope_ids().count(), 2);
    assert_eq!(
        index.scope(reactive_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Lazy)
    );
}

#[test]
fn test_testthat_helper_definition_shadows() {
    // A `helper*.R` file defines `local`, so the test's bare `local` resolves
    // to that helper definition rather than base's NSE `local`. No NSE scope.
    let mut db = TestDb::new();
    let files = setup_testthat(&mut db, &[
        ("pkg/tests/testthat/helper-x.R", "local <- function(x) x\n"),
        ("pkg/tests/testthat/test-a.R", "local({\n    y <- 1\n})\n"),
    ]);

    let index = files[1].semantic_index(&db);
    assert_eq!(index.scope_ids().count(), 1);
}

#[test]
fn test_namespace_import_from_enables_nse() {
    // `importFrom(shiny, reactive)` brings `reactive` into the package
    // namespace, so a bare `reactive` call resolves to shiny's NSE annotation
    // with no `library()`.
    let mut db = TestDb::new();
    install_packages(&mut db, &["shiny"]);
    let root = workspace_root(&db, "ws");
    let namespace = Namespace {
        imports: vec![Import {
            name: "reactive".to_string(),
            package: "shiny".to_string(),
        }],
        ..Default::default()
    };
    let (pkg, files) = make_package(&mut db, "pkg", namespace, &[(
        "ws/pkg/R/a.R",
        "reactive({\n    x <- 1\n})\n",
    )]);
    root.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let index = files[0].semantic_index(&db);
    let reactive_scope = ScopeId::from(1);
    assert_eq!(index.scope_ids().count(), 2);
    assert_eq!(
        index.scope(reactive_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Lazy)
    );
}

#[test]
fn test_namespace_bulk_import_enables_nse() {
    // `import(shiny)` makes all of shiny's exports visible. shiny exports
    // `reactive`, so the bare call is NSE. The registry supplies the effect.
    let mut db = TestDb::new();
    let root = workspace_root(&db, "ws");

    let caller_ns = Namespace {
        package_imports: vec!["shiny".to_string()],
        ..Default::default()
    };
    let (caller, files) = make_package(&mut db, "caller", caller_ns, &[(
        "ws/caller/R/a.R",
        "reactive({\n    x <- 1\n})\n",
    )]);

    let shiny_ns = Namespace {
        exports: SortedVec::from_vec(vec!["reactive".to_string()]),
        ..Default::default()
    };
    let (shiny, _) = make_package(&mut db, "shiny", shiny_ns, &[]);

    root.set_packages(&mut db).to(vec![caller, shiny]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let index = files[0].semantic_index(&db);
    let reactive_scope = ScopeId::from(1);
    assert_eq!(index.scope_ids().count(), 2);
    assert_eq!(
        index.scope(reactive_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Lazy)
    );
}

#[test]
fn test_namespace_reexport_chases_to_source_package() {
    // `caller` imports `reactive` from `mypkg`, which re-exports it from shiny
    // (`importFrom(shiny, reactive)`). The NSE annotation lives under shiny, so
    // resolution follows one hop through `mypkg`'s import to `shiny`.
    let mut db = TestDb::new();
    let root = workspace_root(&db, "ws");

    let caller_ns = Namespace {
        imports: vec![Import {
            name: "reactive".to_string(),
            package: "mypkg".to_string(),
        }],
        ..Default::default()
    };
    let (caller, files) = make_package(&mut db, "caller", caller_ns, &[(
        "ws/caller/R/a.R",
        "reactive({\n    x <- 1\n})\n",
    )]);

    let mypkg_ns = Namespace {
        exports: SortedVec::from_vec(vec!["reactive".to_string()]),
        imports: vec![Import {
            name: "reactive".to_string(),
            package: "shiny".to_string(),
        }],
        ..Default::default()
    };
    let (mypkg, _) = make_package(&mut db, "mypkg", mypkg_ns, &[]);

    root.set_packages(&mut db).to(vec![caller, mypkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let index = files[0].semantic_index(&db);
    let reactive_scope = ScopeId::from(1);
    assert_eq!(index.scope_ids().count(), 2);
    assert_eq!(
        index.scope(reactive_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Lazy)
    );
}

#[test]
fn test_namespace_reexport_requires_export() {
    // `caller` importFroms `reactive` from `mypkg`, but `mypkg` only importFroms
    // it from shiny without re-exporting it. mypkg doesn't hand `reactive` to
    // its importers (R errors "could not find function"), so the bare call is
    // not NSE. The re-export chase is gated on mypkg's own exports, same as
    // `Package::resolve`.
    let mut db = TestDb::new();
    let root = workspace_root(&db, "ws");

    let caller_ns = Namespace {
        imports: vec![Import {
            name: "reactive".to_string(),
            package: "mypkg".to_string(),
        }],
        ..Default::default()
    };
    let (caller, files) = make_package(&mut db, "caller", caller_ns, &[(
        "ws/caller/R/a.R",
        "reactive({\n    x <- 1\n})\n",
    )]);

    // mypkg imports reactive from shiny but does NOT export it.
    let mypkg_ns = Namespace {
        imports: vec![Import {
            name: "reactive".to_string(),
            package: "shiny".to_string(),
        }],
        ..Default::default()
    };
    let (mypkg, _) = make_package(&mut db, "mypkg", mypkg_ns, &[]);

    root.set_packages(&mut db).to(vec![caller, mypkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let index = files[0].semantic_index(&db);
    assert_eq!(index.scope_ids().count(), 1);
}

#[test]
fn test_testthat_namespace_import_enables_nse() {
    // A test file runs in a child of the package's namespace, so the package's
    // `importFrom(shiny, reactive)` is visible to the test body: a bare
    // `reactive` is NSE without any `library()`.
    let mut db = TestDb::new();
    install_packages(&mut db, &["shiny"]);
    let root = workspace_root(&db, "pkg");
    let namespace = Namespace {
        imports: vec![Import {
            name: "reactive".to_string(),
            package: "shiny".to_string(),
        }],
        ..Default::default()
    };
    let pkg = Package::new(
        &db,
        file_path("pkg/DESCRIPTION"),
        "pkg".to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        Some(namespace),
        Vec::new(),
        Vec::new(),
    );
    let test = File::new(
        &db,
        file_path("pkg/tests/testthat/test-a.R"),
        FileRevision::zero(),
        Some("reactive({\n    x <- 1\n})\n".to_string()),
        Some(pkg),
    );
    pkg.set_scripts(&mut db).to(vec![test]);
    root.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let index = test.semantic_index(&db);
    let reactive_scope = ScopeId::from(1);
    assert_eq!(index.scope_ids().count(), 2);
    assert_eq!(
        index.scope(reactive_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Lazy)
    );
}

#[test]
fn test_sibling_definition_shadows_attached_nse() {
    // `a.R` defines `reactive`, so in the later `b.R` the bare `reactive` call
    // resolves to that sibling definition, not shiny's NSE function, even
    // though `b.R` attaches shiny. A collation predecessor's binding beats the
    // attached search path, so no NSE scope is pushed.
    let mut db = TestDb::new();
    let files = setup_package(&mut db, &[
        ("pkg/R/a.R", "reactive <- function(x) x\n"),
        ("pkg/R/b.R", "library(shiny)\nreactive({\n    x <- 1\n})\n"),
    ]);
    let b = files[1];

    let index = b.semantic_index(&db);
    assert_eq!(index.scope_ids().count(), 1);
}

#[test]
fn test_namespace_import_shadows_attached_nse() {
    // The package importFroms `reactive` from `dep`, which exports it as a plain
    // function, and `a.R` also attaches shiny, whose `reactive` is NSE. R
    // searches the namespace before the attached search path, so the plain
    // `dep::reactive` binds the name and shadows shiny's NSE one. Export-driven
    // shadowing: the namespace import stops the walk even though it has no
    // effect, so the bare call is not NSE.
    let mut db = TestDb::new();
    install_packages(&mut db, &["shiny"]);
    let root = workspace_root(&db, "ws");
    let namespace = Namespace {
        imports: vec![Import {
            name: "reactive".to_string(),
            package: "dep".to_string(),
        }],
        ..Default::default()
    };
    let (pkg, files) = make_package(&mut db, "pkg", namespace, &[(
        "ws/pkg/R/a.R",
        "library(shiny)\nreactive({\n    x <- 1\n})\n",
    )]);
    let dep_ns = Namespace {
        exports: SortedVec::from_vec(vec!["reactive".to_string()]),
        ..Default::default()
    };
    let (dep, _) = make_package(&mut db, "dep", dep_ns, &[(
        "ws/dep/R/z.R",
        "reactive <- function(x) x\n",
    )]);
    root.set_packages(&mut db).to(vec![pkg, dep]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let index = files[0].semantic_index(&db);
    assert_eq!(index.scope_ids().count(), 1);
}

#[test]
fn test_higher_attach_plain_export_shadows_lower_nse() {
    // `a.R` attaches shiny (NSE `reactive`) then `dep`, which exports `reactive`
    // as a plain function. `dep` is attached last, so it's highest on the search
    // path and its plain `reactive` binds the name, shadowing shiny's NSE one.
    // The bare call is not NSE.
    let mut db = TestDb::new();
    install_packages(&mut db, &["shiny"]);
    let root = workspace_root(&db, "ws");
    let dep_ns = Namespace {
        exports: SortedVec::from_vec(vec!["reactive".to_string()]),
        ..Default::default()
    };
    let (dep, _) = make_package(&mut db, "dep", dep_ns, &[(
        "ws/dep/R/z.R",
        "reactive <- function(x) x\n",
    )]);
    let (pkg, files) = make_package(&mut db, "pkg", Namespace::default(), &[(
        "ws/pkg/R/a.R",
        "library(shiny)\nlibrary(dep)\nreactive({\n    x <- 1\n})\n",
    )]);
    root.set_packages(&mut db).to(vec![pkg, dep]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let index = files[0].semantic_index(&db);
    assert_eq!(index.scope_ids().count(), 1);
}

#[test]
fn test_later_sibling_does_not_shadow() {
    // `b.R` defines `reactive`, but it's a collation successor of `a.R`, so it
    // hasn't loaded when `a.R` runs. `a.R`'s bare `reactive` still resolves to
    // shiny (attached in `a.R`) and stays NSE. The predecessors-only rule.
    let mut db = TestDb::new();
    install_packages(&mut db, &["shiny"]);
    let files = setup_package(&mut db, &[
        ("pkg/R/a.R", "library(shiny)\nreactive({\n    x <- 1\n})\n"),
        ("pkg/R/b.R", "reactive <- function(x) x\n"),
    ]);
    let a = files[0];

    let index = a.semantic_index(&db);
    let reactive_scope = ScopeId::from(1);
    assert_eq!(index.scope_ids().count(), 2);
    assert_eq!(
        index.scope(reactive_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Lazy)
    );
}

#[test]
fn test_predecessor_sibling_attach_enables_nse() {
    // `a.R` attaches shiny at top level. By the time `b.R` loads shiny is on
    // the search path, so `b.R`'s bare `reactive` is NSE without a `library()`
    // of its own.
    let mut db = TestDb::new();
    install_packages(&mut db, &["shiny"]);
    let files = setup_package(&mut db, &[
        ("pkg/R/a.R", "library(shiny)\n"),
        ("pkg/R/b.R", "reactive({\n    x <- 1\n})\n"),
    ]);
    let b = files[1];

    let index = b.semantic_index(&db);
    let reactive_scope = ScopeId::from(1);
    assert_eq!(index.scope_ids().count(), 2);
    assert_eq!(
        index.scope(reactive_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Lazy)
    );
}

#[test]
fn test_cross_file_source_injection() {
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(&mut db, &[
        ("a.R", "source(\"b.R\")\n"),
        ("b.R", "x <- 1\n"),
    ]);
    let (a, b) = (scripts[0], scripts[1]);

    let index = a.semantic_index(&db);
    let file_scope = ScopeId::from(0);

    let exports = index.exports();
    assert!(exports.contains_key("x"));

    let import_def = index
        .definitions(file_scope)
        .iter()
        .find(|(_, def)| matches!(def.kind(), DefinitionKind::Import { .. }));
    assert!(import_def.is_some());

    match import_def.unwrap().1.kind() {
        DefinitionKind::Import { file, name, .. } => {
            assert_eq!(file, &b.path(&db).to_url());
            assert_eq!(name, "x");
        },
        _ => unreachable!(),
    }
}

#[test]
fn test_editing_sourced_file_invalidates_caller_index() {
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(&mut db, &[
        ("a.R", "source(\"b.R\")\n"),
        ("b.R", "x <- 1\n"),
    ]);
    let (a, b) = (scripts[0], scripts[1]);

    let _ = a.semantic_index(&db);
    assert_eq!(db.executions("semantic_index"), 2);

    // Add a new top-level definition in `b`. `a` sees `b`'s exports
    // change, so its index must re-run.
    b.set_source_text_override(&mut db)
        .to(Some("x <- 1\ny <- 2\n".to_string()));
    let _ = a.semantic_index(&db);
    // 4 = 2 initial (a + b) + 2 re-runs (b's parse and index invalidate
    // first via the contents bump, then a's index re-runs because its
    // dep on b's index lost validity).
    assert_eq!(db.executions("semantic_index"), 4);

    let index = a.semantic_index(&db);
    let exports = index.exports();
    assert!(exports.contains_key("x"));
    assert!(exports.contains_key("y"));
}

#[test]
fn test_source_cycle_preserves_local_analysis() {
    // `a` sources `b`, `b` sources `a`. Salsa breaks the cycle by
    // rebuilding one side with `NoopImportsResolver`, so that side keeps its
    // own local definitions but loses the cross-file imports from the
    // cycle partner. The other side completes normally.
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(&mut db, &[
        ("a.R", "source(\"b.R\")\nx_a <- 1\n"),
        ("b.R", "source(\"a.R\")\nx_b <- 2\n"),
    ]);
    let (a, b) = (scripts[0], scripts[1]);

    let index_a = a.semantic_index(&db);
    let index_b = b.semantic_index(&db);

    // Both files keep their own local binding regardless of which side
    // salsa picks as the cycle break point.
    assert!(index_a.exports().contains_key("x_a"));
    assert!(index_b.exports().contains_key("x_b"));
}

#[test]
fn test_library_in_sourced_file_records_attach_call() {
    // R runtime: `source("helpers.R")` runs every top-level statement
    // in `helpers.R`, including its `library()` calls. Those attaches
    // persist in the caller's search path, so `SalsaImportsResolver`
    // plumbs the sourced file's `file_attached_packages` through
    // `SourceResolution` and the builder re-records them as `Attach`
    // semantic calls at the `source()` call's offset.
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(&mut db, &[
        ("helpers.R", "library(dplyr)\n"),
        ("analysis.R", "source(\"helpers.R\")\n"),
    ]);
    let analysis = scripts[1];

    let index = analysis.semantic_index(&db);
    let attaches: Vec<&str> = index
        .semantic_calls()
        .iter()
        .filter_map(|c| match c.kind() {
            SemanticCallKind::Attach { package } => Some(package.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(attaches, vec!["dplyr"]);
}

#[test]
fn test_library_propagates_transitively_through_source_chains() {
    // a sources b sources c; c does `library(dplyr)`. Each hop's
    // `SalsaImportsResolver` pulls the previous file's attaches through
    // `file_attached_packages`, so `dplyr` ends up recorded as an
    // attach in `a`'s semantic_index.
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(&mut db, &[
        ("c.R", "library(dplyr)\n"),
        ("b.R", "source(\"c.R\")\n"),
        ("a.R", "source(\"b.R\")\n"),
    ]);
    let a = scripts[2];

    let index = a.semantic_index(&db);
    let attaches: Vec<&str> = index
        .semantic_calls()
        .iter()
        .filter_map(|c| match c.kind() {
            SemanticCallKind::Attach { package } => Some(package.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(attaches, vec!["dplyr"]);
}

#[test]
fn test_closure_capture_with_source_before_function() {
    // source() comes first, so by the time `f`'s body is walked the
    // file-scope symbol table already has `helper` flagged
    // `IS_BOUND` via the injected Import. The free-variable lookup
    // inside `f` finds it through the existing enclosing-snapshot
    // machinery, no pre-scan needed.
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(&mut db, &[
        (
            "script.R",
            "source(\"helpers.R\")\nf <- function() helper\n",
        ),
        ("helpers.R", "helper <- 1\n"),
    ]);
    let script = scripts[0];

    let index = script.semantic_index(&db);
    let file_scope = ScopeId::from(0);
    let fn_scope = ScopeId::from(1);

    // The function body's lone use is `helper`. Its enclosing snapshot
    // should point at the file scope and the bindings should be
    // non-empty (containing the Import).
    let fn_map = index.use_def_map(fn_scope);
    let use_id = oak_semantic::UseId::from(0);
    let bindings = fn_map.bindings_at_use(use_id);
    assert!(bindings.may_be_unbound());

    let (enclosing_scope, enclosing_bindings) = index
        .enclosing_bindings(fn_scope, use_id)
        .expect("`helper` should have an enclosing snapshot at the file scope");
    assert_eq!(enclosing_scope, file_scope);
    assert!(!enclosing_bindings.definitions().is_empty());

    // The enclosing binding is the Import injected by source().
    let def_id = enclosing_bindings.definitions()[0];
    let def = &index.definitions(file_scope)[def_id];
    assert!(matches!(def.kind(), DefinitionKind::Import { .. }));
}

#[test]
fn test_sourced_file_library_attaches_in_caller() {
    // `b.R` calls `library(foo)`. After `a.R` sources `b.R`, the
    // resolver carries `foo` into `a`'s attached-packages set, so a
    // scope query against `a` sees the same packages it would see if
    // the `library(foo)` call had appeared directly in `a`.
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(&mut db, &[
        ("a.R", "source(\"b.R\")\n"),
        ("b.R", "library(foo)\n"),
    ]);
    let a = scripts[0];

    let index = a.semantic_index(&db);
    assert!(index.attached_packages().contains(&"foo"));
}

#[test]
fn test_attached_package_enables_nse_scope() {
    // `library(shiny)` attaches shiny, so the eager `reactive` resolves through
    // `SalsaImportsResolver::resolve_effects` walking the `attached` set to
    // shiny's NSE annotation and pushes a lazy nested scope. Base-only
    // resolution would miss it.
    let mut db = TestDb::new();
    install_packages(&mut db, &["shiny"]);
    let (_, scripts) = setup_workspace(&mut db, &[(
        "app.R",
        "library(shiny)\nreactive({\n    x <- 1\n})\n",
    )]);
    let app = scripts[0];

    let index = app.semantic_index(&db);
    let reactive_scope = ScopeId::from(1);
    assert_eq!(index.scope_ids().count(), 2);
    assert_eq!(
        index.scope(reactive_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Lazy)
    );
}

#[test]
fn test_base_nse_resolves_with_no_attaches() {
    // No attaches, so the search-path walk falls straight through to base:
    // `local` is a base NSE function and still pushes its nested scope. Guards
    // the base fallback through the new attached-walk code path.
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(&mut db, &[("s.R", "local({\n    x <- 1\n})\n")]);
    let s = scripts[0];

    let index = s.semantic_index(&db);
    let local_scope = ScopeId::from(1);
    assert_eq!(index.scope_ids().count(), 2);
    assert_eq!(
        index.scope(local_scope).kind(),
        ScopeKind::Nse(NseScope::Nested, NseTiming::Eager)
    );
}

#[test]
fn test_lazy_library_in_sourced_file_does_not_attach_in_caller() {
    // `b.R` calls `library(shiny)` inside a function body, so it attaches only
    // if that function runs. Sourcing `b.R` defines the function but never
    // calls it, so `shiny` must not be forwarded into the caller. `b.R`'s
    // eager top-level `library(dplyr)` does forward.
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(&mut db, &[
        ("a.R", "source(\"b.R\")\n"),
        ("b.R", "library(dplyr)\nf <- function() library(shiny)\n"),
    ]);
    let a = scripts[0];

    let index = a.semantic_index(&db);
    let attaches = index.attached_packages();
    assert!(attaches.contains(&"dplyr"));
    assert!(!attaches.contains(&"shiny"));
}

#[test]
fn test_source_to_unregistered_url_resolves_to_none() {
    // `a.R` sources `b.R` but `b.R` isn't registered. The `Source`
    // semantic call is still recorded so diagnostics can flag the
    // unresolved import; no `Import` definition lands in `a`'s file
    // scope.
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(&mut db, &[("a.R", "source(\"b.R\")\n")]);
    let a = scripts[0];

    let index = a.semantic_index(&db);

    let imports = index
        .definitions(ScopeId::from(0))
        .iter()
        .any(|(_, def)| matches!(def.kind(), DefinitionKind::Import { .. }));
    assert!(!imports);

    let source_calls: Vec<_> = index
        .semantic_calls()
        .iter()
        .filter_map(|c| match c.kind() {
            SemanticCallKind::Source { path, resolved } => Some((path.as_str(), resolved)),
            _ => None,
        })
        .collect();
    assert_eq!(source_calls, [("b.R", &None)]);
}

#[test]
fn test_source_resolves_absolute_path() {
    // `source("/abs/b.R")` joins to an absolute path regardless of
    // `a`'s parent directory. The target URL is reconstructed
    // unambiguously and the registered script at that URL is found.
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(&mut db, &[
        ("a.R", "source(\"/abs/b.R\")\n"),
        ("abs/b.R", "x <- 1\n"),
    ]);
    let a = scripts[0];

    let index = a.semantic_index(&db);
    assert!(index.exports().contains_key("x"));
}

#[test]
fn test_source_chain_propagates_exports_transitively() {
    // a sources b, b sources c, c defines x_c. Each Import is recorded
    // at its `source()` call site, and `file_exports` walks them all
    // out, so a sees x_a, x_b (forwarded from b), and x_c (forwarded
    // from b which forwarded it from c).
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(&mut db, &[
        ("a.R", "source(\"b.R\")\nx_a <- 1\n"),
        ("b.R", "source(\"c.R\")\nx_b <- 2\n"),
        ("c.R", "x_c <- 3\n"),
    ]);
    let a = scripts[0];

    let exports = a.semantic_index(&db).exports();
    assert!(exports.contains_key("x_a"));
    assert!(exports.contains_key("x_b"));
    assert!(exports.contains_key("x_c"));
}

#[test]
#[ignore = "known limitation: pre-scan does not yet detect `source()` injection. \
            When source() follows the function definition, the function's free-variable \
            lookup runs before the Import lands in the file scope, so the enclosing \
            snapshot misses it. Fixing this requires extending the pre-scan to consult \
            the resolver for source() / library() targets -- the same extension NSE \
            scope resolution needs to detect imported NSE call targets that are \
            brought in by source() / library() later in the file. TODO(nse)"]
fn test_closure_capture_with_source_after_function() {
    // Function defined first, source() injected after. The walk
    // processes `f`'s body before the source() call, so when
    // `register_enclosing_snapshot` looks up `helper` in the file
    // scope, neither the symbol table nor the pre-scan knows about it
    // yet, and the snapshot doesn't register.
    let mut db = TestDb::new();
    let (_, scripts) = setup_workspace(&mut db, &[
        (
            "script.R",
            "f <- function() helper\nsource(\"helpers.R\")\n",
        ),
        ("helpers.R", "helper <- 1\n"),
    ]);
    let script = scripts[0];

    let index = script.semantic_index(&db);
    let fn_scope = ScopeId::from(1);

    let use_id = oak_semantic::UseId::from(0);
    assert!(index.enclosing_bindings(fn_scope, use_id).is_some());
}

#[test]
fn test_source_anchors_relative_to_workspace_root() {
    // Calling file sits in a subdir of the workspace. `source("b.R")`
    // anchors against the workspace root, not against the calling
    // file's directory: the target is `proj/b.R`, not `proj/sub/b.R`.
    // Matches R's `getwd()` semantics under RStudio / Positron, where
    // the project root is the working directory.
    let mut db = TestDb::new();
    let root = workspace_root(&db, "proj");
    let a = File::new(
        &db,
        file_path("proj/sub/a.R"),
        FileRevision::zero(),
        Some("source(\"b.R\")\n".to_string()),
        None,
    );
    let b = File::new(
        &db,
        file_path("proj/b.R"),
        FileRevision::zero(),
        Some("x <- 1\n".to_string()),
        None,
    );
    root.set_scripts(&mut db).to(vec![a, b]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let index = a.semantic_index(&db);
    assert!(index.exports().contains_key("x"));
}

#[test]
fn test_source_anchors_to_parent_dir_when_no_workspace() {
    // Calling file isn't under any workspace root, so the anchor
    // falls back to the file's own parent directory.
    let mut db = TestDb::new();
    let a = File::new(
        &db,
        file_path("dir/a.R"),
        FileRevision::zero(),
        Some("source(\"b.R\")\n".to_string()),
        None,
    );
    let b = File::new(
        &db,
        file_path("dir/b.R"),
        FileRevision::zero(),
        Some("x <- 1\n".to_string()),
        None,
    );
    db.orphan_root()
        .set_files(&mut db)
        .to(HashSet::from([a, b]));

    let index = a.semantic_index(&db);
    assert!(index.exports().contains_key("x"));
}

#[test]
fn test_source_path_with_parent_dir_segments() {
    // `source("../b.R")` from `dir/sub/a.R` normalises to `dir/b.R`.
    // Exercises the `..` handling in `normalise_path`.
    let mut db = TestDb::new();
    let a = File::new(
        &db,
        file_path("dir/sub/a.R"),
        FileRevision::zero(),
        Some("source(\"../b.R\")\n".to_string()),
        None,
    );
    let b = File::new(
        &db,
        file_path("dir/b.R"),
        FileRevision::zero(),
        Some("x <- 1\n".to_string()),
        None,
    );
    db.orphan_root()
        .set_files(&mut db)
        .to(HashSet::from([a, b]));

    let index = a.semantic_index(&db);
    assert!(index.exports().contains_key("x"));
}
