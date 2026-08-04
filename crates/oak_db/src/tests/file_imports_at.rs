use biome_rowan::TextSize;
use salsa::Setter;

use crate::search::DEFAULT_SEARCH_PATH_PACKAGES;
use crate::tests::test_db::file_path;
use crate::tests::test_db::library_root;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::DbInputs;
use crate::File;
use crate::FileRevision;
use crate::ImportLayer;
use crate::Package;

fn make_file(db: &mut TestDb, path: &str, contents: &str) -> File {
    File::new(
        db,
        file_path(path),
        FileRevision::zero(),
        Some(contents.to_string()),
        None,
    )
}

fn make_package_file(db: &mut TestDb, path: &str, contents: &str, package: Package) -> File {
    File::new(
        db,
        file_path(path),
        FileRevision::zero(),
        Some(contents.to_string()),
        Some(package),
    )
}

/// Register a set of installed packages on `LibraryRoots`, one library
/// root per package. Returns the packages in input order.
fn install_packages(db: &mut TestDb, names: &[&str]) -> Vec<Package> {
    let mut roots = Vec::new();
    let mut packages = Vec::new();
    for &name in names {
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
        roots.push(root);
        packages.push(pkg);
    }
    db.library_roots().set_roots(db).to(roots);
    packages
}

/// Create a workspace package under a fresh `workspace/{name}` root and
/// register the root on `WorkspaceRoots`. Returns the package.
fn install_workspace_package(db: &mut TestDb, name: &str) -> Package {
    let root = workspace_root(db, &format!("workspace/{name}"));
    let pkg = Package::new(
        db,
        file_path(&format!("workspace/{name}/DESCRIPTION")),
        name.to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        None,
        Vec::new(),
        Vec::new(),
    );
    root.set_packages(db).to(vec![pkg]);
    db.workspace_roots().set_roots(db).to(vec![root]);
    pkg
}

/// The package names of all `Package` layers, in layer order.
fn attach_names(db: &TestDb, layers: &[ImportLayer]) -> Vec<String> {
    layers
        .iter()
        .filter_map(|layer| match layer {
            ImportLayer::Package(p) => Some(p.name(db).to_string()),
            _ => None,
        })
        .collect()
}

/// User `library()` / `require()` attaches, with the always-present default
/// search path dropped so a narrowing assertion sees only what the cursor's
/// position changes.
fn library_attaches(db: &TestDb, layers: &[ImportLayer]) -> Vec<String> {
    attach_names(db, layers)
        .into_iter()
        .filter(|name| !DEFAULT_SEARCH_PATH_PACKAGES.contains(&name.as_str()))
        .collect()
}

fn package_files(layers: &[ImportLayer]) -> Vec<File> {
    layers
        .iter()
        .filter_map(|layer| match layer {
            ImportLayer::File(file) | ImportLayer::SourcingFile { file, .. } => Some(*file),
            _ => None,
        })
        .collect()
}

/// Layer identities, sorted and deduped, for comparing two views that cover the
/// same layers in different orders.
fn layer_keys(db: &TestDb, layers: &[ImportLayer]) -> Vec<String> {
    let mut keys: Vec<String> = layers.iter().map(|layer| layer_key(db, layer)).collect();
    keys.sort();
    keys.dedup();
    keys
}

fn layer_key(db: &TestDb, layer: &ImportLayer) -> String {
    match layer {
        ImportLayer::File(file) => format!("File({})", file.path(db)),
        ImportLayer::SourcingFile {
            file,
            exports_so_far,
        } => {
            let mut names: Vec<&str> = exports_so_far.iter().map(String::as_str).collect();
            names.sort();
            format!("SourcingFile({}, {names:?})", file.path(db))
        },
        ImportLayer::Package(package) => format!("Package({})", package.name(db)),
        ImportLayer::From(package) => format!("From({})", package.name(db)),
    }
}

#[test]
fn test_script_cursor_before_any_attach_sees_no_attached_packages() {
    let mut db = TestDb::new();
    let packages = install_packages(&mut db, &["dplyr", "ggplot2"]);
    let _ = packages;

    let source = "library(dplyr)\nlibrary(ggplot2)\nx <- 1\n";
    let file = make_file(&mut db, "/a.R", source);

    let layers = file.imports_at(&db, TextSize::from(0));
    assert_eq!(library_attaches(&db, &layers), Vec::<String>::new());
}

#[test]
fn test_script_cursor_after_all_attaches_sees_all_in_lifo_order() {
    let mut db = TestDb::new();
    install_packages(&mut db, &["dplyr", "ggplot2"]);

    let source = "library(dplyr)\nlibrary(ggplot2)\nx <- 1\n";
    let file = make_file(&mut db, "/a.R", source);

    let offset = TextSize::from(source.len() as u32);
    let layers = file.imports_at(&db, offset);
    // LIFO: latest `library()` call comes first.
    assert_eq!(library_attaches(&db, &layers), vec![
        "ggplot2".to_string(),
        "dplyr".to_string()
    ]);
}

#[test]
fn test_script_cursor_between_attaches_sees_only_earlier_ones() {
    let mut db = TestDb::new();
    install_packages(&mut db, &["dplyr", "ggplot2"]);

    let source = "library(dplyr)\nlibrary(ggplot2)\nx <- 1\n";
    let file = make_file(&mut db, "/a.R", source);

    let offset = TextSize::from(source.find("library(ggplot2)").unwrap() as u32);
    let layers = file.imports_at(&db, offset);
    assert_eq!(library_attaches(&db, &layers), vec!["dplyr".to_string()]);
}

#[test]
fn test_function_body_sees_file_scope_attaches_even_if_after_function_in_source() {
    // R's runtime. File-scope `library()` calls run before any function
    // body executes, so the function sees the package regardless of source
    // order. The offset filter must override its "before cursor" rule for
    // file-scope attaches when the cursor is inside a function body.
    let mut db = TestDb::new();
    install_packages(&mut db, &["dplyr"]);

    let source = "f <- function() {\n  x\n}\nlibrary(dplyr)\n";
    let file = make_file(&mut db, "/a.R", source);

    let offset = TextSize::from(source.find("x\n}").unwrap() as u32);
    let layers = file.imports_at(&db, offset);
    assert!(library_attaches(&db, &layers).contains(&"dplyr".to_string()));
}

#[test]
fn test_package_top_level_sees_predecessor_files_only() {
    let mut db = TestDb::new();
    let _ = install_packages(&mut db, &["base"]);
    let pkg = install_workspace_package(&mut db, "pkg");

    let a = make_package_file(&mut db, "/w/pkg/R/a.R", "first <- 1\n", pkg);
    let b_source = "x <- 1\n";
    let b = make_package_file(&mut db, "/w/pkg/R/b.R", b_source, pkg);
    let c = make_package_file(&mut db, "/w/pkg/R/c.R", "second <- 2\n", pkg);
    pkg.set_files(&mut db).to(vec![a, b, c]);

    // Cursor at top-level in b. Only a (the predecessor in `Package.files`)
    // is visible.
    let offset = TextSize::from(b_source.find('x').unwrap() as u32);
    let layers = b.imports_at(&db, offset);
    assert_eq!(package_files(&layers), vec![a]);
}

#[test]
fn test_package_function_body_sees_other_package_files_in_lifo_order() {
    let mut db = TestDb::new();
    let _ = install_packages(&mut db, &["base"]);
    let pkg = install_workspace_package(&mut db, "pkg");

    let a = make_package_file(&mut db, "/w/pkg/R/a.R", "first <- 1\n", pkg);
    let b_source = "f <- function() {\n  x\n}\n";
    let b = make_package_file(&mut db, "/w/pkg/R/b.R", b_source, pkg);
    let c = make_package_file(&mut db, "/w/pkg/R/c.R", "second <- 2\n", pkg);
    pkg.set_files(&mut db).to(vec![a, b, c]);

    // Cursor inside f's body. Full lazy view (same as `imports()`).
    // Other package files appear in LIFO order. Self (b) is excluded
    // since its own top-level bindings live in `exports`.
    let offset = TextSize::from(b_source.find("x\n}").unwrap() as u32);
    let layers = b.imports_at(&db, offset);
    assert_eq!(package_files(&layers), vec![c, a]);
}

#[test]
fn test_package_top_level_predecessors_appear_in_lifo_order() {
    // Multiple predecessors of the cursor's file appear latest-first
    // (LIFO), matching R's namespace where the most recently sourced
    // file's bindings win.
    let mut db = TestDb::new();
    let _ = install_packages(&mut db, &["base"]);
    let pkg = install_workspace_package(&mut db, "pkg");

    let a = make_package_file(&mut db, "/w/pkg/R/a.R", "first <- 1\n", pkg);
    let b = make_package_file(&mut db, "/w/pkg/R/b.R", "second <- 2\n", pkg);
    let c_source = "x <- 1\n";
    let c = make_package_file(&mut db, "/w/pkg/R/c.R", c_source, pkg);
    pkg.set_files(&mut db).to(vec![a, b, c]);

    let offset = TextSize::from(c_source.find('x').unwrap() as u32);
    let layers = c.imports_at(&db, offset);
    // Predecessors of c are [a, b] in declaration order. LIFO gives [b, a].
    assert_eq!(package_files(&layers), vec![b, a]);
}

#[test]
fn test_package_namespace_and_base_layers_always_visible() {
    let mut db = TestDb::new();
    install_packages(&mut db, &["base"]);
    let pkg = install_workspace_package(&mut db, "pkg");

    let file = make_package_file(&mut db, "/w/pkg/R/a.R", "x <- 1\n", pkg);
    pkg.set_files(&mut db).to(vec![file]);

    let layers = file.imports_at(&db, TextSize::from(0));
    assert!(attach_names(&db, &layers).contains(&"base".to_string()));
}

#[test]
fn test_package_script_top_level_resolves_as_standalone_script() {
    // A `data-raw/` file carries a package back-pointer but lives in
    // `scripts`, not `files`. At top level it must narrow like a standalone
    // script and never take the package path, which would log the spurious
    // "back-pointer but not in its files" warning from #1270.
    let mut db = TestDb::new();
    install_packages(&mut db, &["dplyr"]);
    let pkg = install_workspace_package(&mut db, "pkg");

    let r_file = make_package_file(&mut db, "/workspace/pkg/R/a.R", "internal <- 1\n", pkg);
    let source = "library(dplyr)\nx <- 1\n";
    let data_raw = make_package_file(&mut db, "/workspace/pkg/data-raw/prep.R", source, pkg);
    pkg.set_files(&mut db).to(vec![r_file]);
    pkg.set_scripts(&mut db).to(vec![data_raw]);

    // Before the `library()` call: nothing attached, and no `R/` `File`
    // layers (the script isn't part of the package's namespace).
    let before = data_raw.imports_at(&db, TextSize::from(0));
    assert_eq!(library_attaches(&db, &before), Vec::<String>::new());
    assert_eq!(package_files(&before), Vec::<File>::new());

    // After the call: dplyr attached, still no `R/` `File` layers.
    let after = data_raw.imports_at(&db, TextSize::from(source.len() as u32));
    assert_eq!(library_attaches(&db, &after), vec!["dplyr".to_string()]);
    assert_eq!(package_files(&after), Vec::<File>::new());
}

#[test]
fn test_testthat_top_level_library_narrows_by_offset() {
    // A test file's own top-level `library()` call narrows like a script's:
    // invisible before the call, visible after. Helpers, the package, and
    // testthat (omitted here) stay visible at any offset.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli"]);
    let pkg = install_workspace_package(&mut db, "pkg");

    let source = "library(cli)\ntest_that('x', expect_true(TRUE))\n";
    let test_file = make_package_file(
        &mut db,
        "workspace/pkg/tests/testthat/test-x.R",
        source,
        pkg,
    );
    pkg.set_scripts(&mut db).to(vec![test_file]);

    let before = test_file.imports_at(&db, TextSize::from(0));
    assert!(!library_attaches(&db, &before).contains(&"cli".to_string()));

    let after = test_file.imports_at(&db, TextSize::from(source.len() as u32));
    assert!(library_attaches(&db, &after).contains(&"cli".to_string()));
}

/// Creates a `tests/testthat/` fixture with three support files and one test.
/// Returns `(helper_a, helper_b, setup_c, test_x)`.
fn testthat_support_workspace(db: &mut TestDb, helper_b: &str) -> (File, File, File, File) {
    let pkg = install_workspace_package(db, "pkg");
    let path = |name: &str| format!("workspace/pkg/tests/testthat/{name}");

    let helper_a = make_package_file(db, &path("helper-a.R"), "a_val <- 1\n", pkg);
    let helper_b = make_package_file(db, &path("helper-b.R"), helper_b, pkg);
    let setup_c = make_package_file(db, &path("setup-c.R"), "c_val <- 3\n", pkg);
    let test_x = make_package_file(db, &path("test-x.R"), "x <- 1\n", pkg);

    pkg.set_scripts(db)
        .to(vec![helper_a, helper_b, setup_c, test_x]);
    (helper_a, helper_b, setup_c, test_x)
}

#[test]
fn test_testthat_support_file_top_level_sees_only_earlier_support_files() {
    // testthat sources support files in lexical order. `helper-b.R` runs before
    // `setup-c.R`, so its top-level code cannot see that file.
    let mut db = TestDb::new();
    let (helper_a, helper_b, _setup_c, _test_x) =
        testthat_support_workspace(&mut db, "b_val <- 2\n");

    let layers = helper_b.imports_at(&db, TextSize::from(0));
    assert_eq!(package_files(&layers), vec![helper_a]);
}

#[test]
fn test_testthat_support_file_body_sees_every_support_file() {
    // The function body runs after every support file is sourced. LIFO lookup
    // therefore puts `setup-c.R` before `helper-a.R`.
    let mut db = TestDb::new();
    let source = "f <- function() {\n  inside\n}\n";
    let (helper_a, helper_b, setup_c, _test_x) = testthat_support_workspace(&mut db, source);

    let offset = TextSize::from(source.find("inside").unwrap() as u32);
    let layers = helper_b.imports_at(&db, offset);
    assert_eq!(package_files(&layers), vec![setup_c, helper_a]);
}

#[test]
fn test_testthat_test_file_top_level_sees_every_support_file() {
    // `test-x.R` runs after every support file, so its eager view retains all of them.
    let mut db = TestDb::new();
    let (helper_a, helper_b, setup_c, test_x) = testthat_support_workspace(&mut db, "b_val <- 2\n");

    let layers = test_x.imports_at(&db, TextSize::from(0));
    assert_eq!(package_files(&layers), vec![setup_c, helper_b, helper_a]);
}

#[test]
fn test_library_in_function_scoped_source_is_visible_only_in_that_function() {
    // A sourced `library()` becomes an `Attach` in `source()`'s calling scope.
    // It appears after `source()` returns, but does not escape the function.
    let mut db = TestDb::new();
    install_packages(&mut db, &["dplyr"]);

    let helpers = make_file(&mut db, "w/helpers.R", "library(dplyr)\n");
    let script_src = "before\nf <- function() {\n  source(\"helpers.R\")\n  inside\n}\nafter\n";
    let script = make_file(&mut db, "w/script.R", script_src);

    let root = workspace_root(&db, "w");
    root.set_scripts(&mut db).to(vec![helpers, script]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let at = |needle: &str| {
        let offset = TextSize::from(script_src.find(needle).unwrap() as u32);
        library_attaches(&db, &script.imports_at(&db, offset))
    };

    assert!(!at("before").contains(&"dplyr".to_string()));
    assert!(!at("source").contains(&"dplyr".to_string()));
    assert!(at("inside").contains(&"dplyr".to_string()));
    assert!(!at("after").contains(&"dplyr".to_string()));
}

/// The attaches visible at each `needle` in `source`, one entry per needle.
fn attaches_at(db: &TestDb, file: File, source: &str, needles: &[&str]) -> Vec<Vec<String>> {
    needles
        .iter()
        .map(|needle| {
            let offset = TextSize::from(source.find(needle).unwrap() as u32);
            library_attaches(db, &file.imports_at(db, offset))
        })
        .collect()
}

#[test]
fn test_conditional_attach_holds_only_inside_its_branch() {
    // `library(cli)` runs only when `cond` is true, so past the branch nothing
    // says cli is attached. It covers its own arm and stops at the closing
    // brace, the same narrowing the scan applies to `attached_so_far`.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli"]);

    let source = "before\nif (cond) {\n  library(cli)\n  inside\n}\nafter\n";
    let file = make_file(&mut db, "a.R", source);

    let no_attach: Vec<String> = Vec::new();
    assert_eq!(
        attaches_at(&db, file, source, &["before", "inside", "after"]),
        vec![no_attach.clone(), vec!["cli".to_string()], no_attach]
    );
}

#[test]
fn test_conditional_attach_does_not_reach_the_sibling_branch() {
    // The arm that attaches ends before the `else` starts, so the alternative
    // resolves against the search path as it was before the `if`.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli"]);

    let source = "if (cond) {\n  library(cli)\n  taken\n} else {\n  other\n}\n";
    let file = make_file(&mut db, "a.R", source);

    assert_eq!(attaches_at(&db, file, source, &["taken", "other"]), vec![
        vec!["cli".to_string()],
        Vec::<String>::new()
    ]);
}

#[test]
fn test_attach_on_both_branches_holds_after_the_if() {
    // Both arms attach `cli`, so the join carries one attach past the `if`.
    // This distinguishes effect regions from treating every attach inside an `if`
    // as conditional.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli"]);

    let source = "if (cond) {\n  library(cli)\n} else {\n  library(cli)\n}\nafter\n";
    let file = make_file(&mut db, "a.R", source);

    assert_eq!(attaches_at(&db, file, source, &["after"]), vec![vec![
        "cli".to_string()
    ]]);
}

#[test]
fn test_attach_on_both_branches_does_not_reach_earlier_uses_in_either_arm() {
    // Each arm's attach applies only after its own call. The joined attach begins
    // at the `else` call, so it does not reach `second`.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli"]);

    let source =
        "if (cond) {\n  first\n  library(cli)\n} else {\n  second\n  library(cli)\n}\nafter\n";
    let file = make_file(&mut db, "a.R", source);

    let no_attach: Vec<String> = Vec::new();
    assert_eq!(
        attaches_at(&db, file, source, &["first", "second", "after"]),
        vec![no_attach.clone(), no_attach, vec!["cli".to_string()]]
    );
}

#[test]
fn test_join_matches_arms_per_package_not_wholesale() {
    // The consequence attaches an extra package. Matching is per package, so cli
    // carries past the `if` while rlang stays capped at the arm that attached it.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli", "rlang"]);

    let source =
        "if (cond) {\n  library(cli)\n  library(rlang)\n  inside\n} else {\n  library(cli)\n}\nafter\n";
    let file = make_file(&mut db, "a.R", source);

    assert_eq!(attaches_at(&db, file, source, &["inside", "after"]), vec![
        vec!["rlang".to_string(), "cli".to_string()],
        vec!["cli".to_string()]
    ]);
}

#[test]
fn test_join_caps_a_package_attached_only_by_the_else_arm() {
    // Mirror of the above with the extra package in the `else`. Being the arm
    // that closes the `if` doesn't carry rlang past it, since the consequence
    // never attached it.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli", "rlang"]);

    let source =
        "if (cond) {\n  library(cli)\n} else {\n  library(cli)\n  library(rlang)\n  inside\n}\nafter\n";
    let file = make_file(&mut db, "a.R", source);

    assert_eq!(attaches_at(&db, file, source, &["inside", "after"]), vec![
        vec!["rlang".to_string(), "cli".to_string()],
        vec!["cli".to_string()]
    ]);
}

#[test]
fn test_join_takes_the_else_arm_order_when_the_arms_attach_in_different_orders() {
    // Both arms attach both packages, so both carry past the `if`, but the arms
    // disagree on which was attached last. One layer order has to stand for both
    // paths, and it's the `else` arm's calls that carry the packages out.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli", "rlang"]);

    let source = "if (cond) {\n  library(cli)\n  library(rlang)\n} else {\n  \
                  library(rlang)\n  library(cli)\n}\nafter\n";
    let file = make_file(&mut db, "a.R", source);

    assert_eq!(attaches_at(&db, file, source, &["after"]), vec![vec![
        "cli".to_string(),
        "rlang".to_string()
    ]]);
}

#[test]
fn test_attach_rejoined_inside_an_arm_carries_through_the_outer_join() {
    // The inner `if` rejoins cli onto its own `else` call, which the outer join
    // then sees as the consequence arm's attach. So the outer `else` carries cli
    // out, and the inner calls stay capped at the arm they ran in.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli"]);

    let source = "if (a) {\n  if (b) library(cli) else library(cli)\n} else {\n  before\n  \
                  library(cli)\n}\nafter\n";
    let file = make_file(&mut db, "a.R", source);

    assert_eq!(attaches_at(&db, file, source, &["before", "after"]), vec![
        Vec::<String>::new(),
        vec!["cli".to_string()]
    ]);
}

#[test]
fn test_attach_on_every_arm_of_an_else_if_chain_holds_after_the_chain() {
    // An `else if` nests a whole `if` in the alternative, so each join sees the
    // arm below it already rejoined. The final `else` closes the outer `if` too,
    // which is what lets its attach carry the chain.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli"]);

    let source = "if (a) {\n  first\n  library(cli)\n} else if (b) {\n  second\n  \
                  library(cli)\n} else {\n  third\n  library(cli)\n}\nafter\n";
    let file = make_file(&mut db, "a.R", source);

    let no_attach: Vec<String> = Vec::new();
    assert_eq!(
        attaches_at(&db, file, source, &["first", "second", "third", "after"]),
        vec![no_attach.clone(), no_attach.clone(), no_attach, vec![
            "cli".to_string()
        ]]
    );
}

#[test]
fn test_attach_on_all_but_one_arm_of_an_else_if_chain_drops_at_the_chain() {
    // One arm without the attach breaks the chain, so nothing holds afterwards
    // even though the last arm attaches. Closing the outer `if` isn't on its own
    // enough to carry an attach past it.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli"]);

    let source = "if (a) {\n  library(cli)\n} else if (b) {\n  middle\n} else {\n  \
                  library(cli)\n}\nafter\n";
    let file = make_file(&mut db, "a.R", source);

    let no_attach: Vec<String> = Vec::new();
    assert_eq!(attaches_at(&db, file, source, &["middle", "after"]), vec![
        no_attach.clone(),
        no_attach
    ]);
}

#[test]
fn test_attach_in_loop_body_does_not_hold_after_the_loop() {
    // An empty sequence means the body never runs, so the attach doesn't
    // survive the loop even though no branch is involved.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli"]);

    let source = "for (i in xs) {\n  library(cli)\n  inside\n}\nafter\n";
    let file = make_file(&mut db, "a.R", source);

    assert_eq!(attaches_at(&db, file, source, &["inside", "after"]), vec![
        vec!["cli".to_string()],
        Vec::<String>::new()
    ]);
}

#[test]
fn test_attach_on_both_branches_inside_a_loop_drops_at_the_loop_join() {
    // Both `if` arms attach `cli`, but a loop body may not run. The attach ends
    // at the loop's closing brace.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli"]);

    let source = "while (cond) {\n  if (x) library(cli) else library(cli)\n  inside\n}\nafter\n";
    let file = make_file(&mut db, "a.R", source);

    assert_eq!(attaches_at(&db, file, source, &["inside", "after"]), vec![
        vec!["cli".to_string()],
        Vec::<String>::new()
    ]);
}

#[test]
fn test_conditional_attach_reaches_only_a_body_defined_in_its_branch() {
    // A lazy body ignores source order, so it sees a file-scope attach wherever
    // that attach sits. A conditional one is different: only a body defined
    // inside the arm is guaranteed to run with the package attached.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli"]);

    let source =
        "if (cond) {\n  library(cli)\n  f <- function() guarded\n}\ng <- function() unguarded\n";
    let file = make_file(&mut db, "a.R", source);

    assert_eq!(
        attaches_at(&db, file, source, &["guarded", "unguarded"]),
        vec![vec!["cli".to_string()], Vec::<String>::new()]
    );
}

#[test]
fn test_conditional_attach_inside_a_body_holds_only_in_its_arm() {
    // The arm narrowing is the same inside a lazy body as at file scope: `taken`
    // runs with cli attached, `after` only might.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli"]);

    let source = "g <- function() {\n  if (cond) {\n    library(cli)\n    taken\n  }\n  after\n}\n";
    let file = make_file(&mut db, "a.R", source);

    assert_eq!(attaches_at(&db, file, source, &["taken", "after"]), vec![
        vec!["cli".to_string()],
        Vec::<String>::new()
    ]);
}

#[test]
fn test_cursor_in_local_narrows_to_calls_that_have_run() {
    // `local()` runs at its call site, so a cursor inside it sees the search
    // path as of that point, not the end-of-file view a function body gets.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli"]);

    let source = "local({\n  inside\n})\nlibrary(cli)\n";
    let file = make_file(&mut db, "a.R", source);

    assert_eq!(attaches_at(&db, file, source, &["inside"]), vec![Vec::<
        String,
    >::new(
    )]);
}

#[test]
fn test_attach_in_local_is_visible_after_the_local() {
    // The block runs during the file's own top-level execution, so its
    // `library()` is on the search path afterwards, the same as one written
    // directly at top level.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli"]);

    let source = "local({\n  library(cli)\n})\nafter\n";
    let file = make_file(&mut db, "a.R", source);

    assert_eq!(attaches_at(&db, file, source, &["after"]), vec![vec![
        "cli".to_string()
    ]]);
}

#[test]
fn test_attach_in_function_body_is_not_visible_after_it() {
    // The body may never run, so its `library()` stays out of the top-level
    // view. Guards the eager-scope widening against swallowing lazy scopes.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli"]);

    let source = "f <- function() {\n  library(cli)\n}\nafter\n";
    let file = make_file(&mut db, "a.R", source);

    assert_eq!(attaches_at(&db, file, source, &["after"]), vec![Vec::<
        String,
    >::new(
    )]);
}

#[test]
fn test_attach_in_a_function_body_is_not_visible_in_a_sibling_body() {
    // `g()` and `h()` can run in either order, so `g()`'s attach does not reach
    // `h()`.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli"]);

    let source = "g <- function() {\n  library(cli)\n}\nh <- function() {\n  inside\n}\n";
    let file = make_file(&mut db, "a.R", source);

    assert_eq!(attaches_at(&db, file, source, &["inside"]), vec![Vec::<
        String,
    >::new(
    )]);
}

#[test]
fn test_attach_from_a_local_block_reaches_the_rest_of_the_body() {
    // `local()` runs during `g()`, so its `library()` reaches code after the
    // block.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli"]);

    let source = "g <- function() {\n  local({ library(cli) })\n  inside\n}\n";
    let file = make_file(&mut db, "a.R", source);

    assert_eq!(attaches_at(&db, file, source, &["inside"]), vec![vec![
        "cli".to_string()
    ]]);
}

#[test]
fn test_attach_later_in_an_enclosing_body_is_not_visible_in_a_nested_body() {
    // `h()` can run before `g()` reaches the later `library()`, so the attach
    // does not reach `h()`.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli"]);

    let source = "g <- function() {\n  h <- function() inside\n  library(cli)\n}\n";
    let file = make_file(&mut db, "a.R", source);

    assert_eq!(attaches_at(&db, file, source, &["inside"]), vec![Vec::<
        String,
    >::new(
    )]);
}

#[test]
fn test_attach_in_a_function_body_is_visible_in_a_body_it_encloses() {
    // `outer()` creates `inner` only after the preceding `library()` runs, so
    // the attach reaches `inner()`.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli"]);

    let source = "outer <- function() {\n  library(cli)\n  inner <- function() inside\n}\n";
    let file = make_file(&mut db, "a.R", source);

    assert_eq!(attaches_at(&db, file, source, &["inside"]), vec![vec![
        "cli".to_string()
    ]]);
}

#[test]
fn test_attach_in_a_function_body_is_not_visible_earlier_in_that_body() {
    // `g()` executes `inside` before its later `library()`, so no attach reaches
    // `inside`.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli"]);

    let source = "g <- function() {\n  inside\n  library(cli)\n}\n";
    let file = make_file(&mut db, "a.R", source);

    assert_eq!(attaches_at(&db, file, source, &["inside"]), vec![Vec::<
        String,
    >::new(
    )]);
}

#[test]
fn test_attach_is_not_visible_on_the_attaching_call() {
    // The attach begins after `library()` returns, so offsets in the multiline
    // call see no attach.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli"]);

    let source = "library(\n  cli\n)\nafter\n";
    let file = make_file(&mut db, "a.R", source);

    assert_eq!(
        attaches_at(&db, file, source, &["library", "cli", "after"]),
        vec![Vec::<String>::new(), Vec::<String>::new(), vec![
            "cli".to_string()
        ]]
    );
}

#[test]
fn test_attach_in_a_function_body_is_visible_later_in_that_body() {
    // `library()` returns before the later `inside` expression runs in `g()`, so
    // the attach is visible there.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli"]);

    let source = "g <- function() {\n  library(cli)\n  inside\n}\n";
    let file = make_file(&mut db, "a.R", source);

    assert_eq!(attaches_at(&db, file, source, &["inside"]), vec![vec![
        "cli".to_string()
    ]]);
}

#[test]
fn test_script_r_directory_top_level_sees_only_alphabetic_predecessor() {
    let mut db = TestDb::new();
    let root = workspace_root(&db, "ws");
    let a = make_file(&mut db, "ws/R/a.R", "a_val <- 1\n");
    let b_source = "x <- 1\n";
    let b = make_file(&mut db, "ws/R/b.R", b_source);
    root.set_scripts(&mut db).to(vec![a, b]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    // `b.R` is alphabetically after `a.R`, so `a.R` is its collation
    // predecessor.
    let offset = TextSize::from(b_source.find('x').unwrap() as u32);
    assert_eq!(package_files(&b.imports_at(&db, offset)), vec![a]);

    // `a.R` has no predecessor: it's first in collation order.
    let offset = TextSize::from(0);
    assert_eq!(
        package_files(&a.imports_at(&db, offset)),
        Vec::<File>::new()
    );
}

#[test]
fn test_script_r_directory_collation_is_case_insensitive() {
    // Matches `oak_scan::packages::order_alphabetically`: basenames sort
    // case-insensitively, so `a.R` collates before `Z.R` even though it's
    // lexically greater in byte order.
    let mut db = TestDb::new();
    let root = workspace_root(&db, "ws");
    let z_source = "z_val <- 1\n";
    let z_file = make_file(&mut db, "ws/R/Z.R", z_source);
    let a_file = make_file(&mut db, "ws/R/a.R", "a_val <- 1\n");
    root.set_scripts(&mut db).to(vec![z_file, a_file]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let offset = TextSize::from(z_source.len() as u32);
    assert_eq!(package_files(&z_file.imports_at(&db, offset)), vec![a_file]);
}

#[test]
fn test_script_r_directory_unplaced_file_still_sees_only_predecessors() {
    // A file the editor opened before the scanner placed it sits in
    // `OrphanRoot`, so it's missing from its own `collation_siblings`. Its
    // collation position comes from its basename anyway, so the top-level view
    // stays the strict predecessor prefix instead of widening to every sibling.
    let mut db = TestDb::new();
    let root = workspace_root(&db, "ws");
    let a = make_file(&mut db, "ws/R/a.R", "a_val <- 1\n");
    let b_source = "x <- 1\n";
    let b = make_file(&mut db, "ws/R/b.R", b_source);
    let c = make_file(&mut db, "ws/R/c.R", "c_val <- 3\n");

    // `b.R` is left out: unscanned, so it isn't a collation sibling of anyone.
    root.set_scripts(&mut db).to(vec![a, c]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let offset = TextSize::from(b_source.find('x').unwrap() as u32);
    assert_eq!(package_files(&b.imports_at(&db, offset)), vec![a]);
}

#[test]
fn test_inherited_attach_is_offset_sensitive_via_source_call_position() {
    // `main.R`'s `library(dplyr)` runs after its `source()` call, so a
    // top-level cursor in `helpers.R` (Eager: attaches up to the source-call
    // offset) doesn't see it, while a cursor in a function body (Lazy:
    // end-of-file view) does.
    let mut db = TestDb::new();
    install_packages(&mut db, &["dplyr"]);
    let root = workspace_root(&db, "w");

    let main_source = "source(\"helpers.R\")\nlibrary(dplyr)\n";
    let main = make_file(&mut db, "w/main.R", main_source);

    let helpers_source = "top <- 1\nf <- function() {\n  body_stmt\n}\n";
    let helpers = make_file(&mut db, "w/helpers.R", helpers_source);

    root.set_scripts(&mut db).to(vec![main, helpers]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let top_offset = TextSize::from(helpers_source.find("top").unwrap() as u32);
    let top_layers = helpers.imports_at(&db, top_offset);
    assert!(!library_attaches(&db, &top_layers).contains(&"dplyr".to_string()));

    let body_offset = TextSize::from(helpers_source.find("body_stmt").unwrap() as u32);
    let body_layers = helpers.imports_at(&db, body_offset);
    assert!(library_attaches(&db, &body_layers).contains(&"dplyr".to_string()));
}

#[test]
fn test_attach_in_eager_scope_is_visible_to_later_top_level_code() {
    // `library()` attaches to the global search path whatever frame it runs in,
    // so a call inside `local()` counts for code after the block exactly like a
    // top-level one would. It's invisible before the block, though, same
    // narrowing as any other attach.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli"]);

    let source = "before\nlocal({\n  library(cli)\n})\nafter\n";
    let file = make_file(&mut db, "a.R", source);

    let before = TextSize::from(source.find("before").unwrap() as u32);
    assert!(!library_attaches(&db, &file.imports_at(&db, before)).contains(&"cli".to_string()));

    let after = TextSize::from(source.find("after").unwrap() as u32);
    assert!(library_attaches(&db, &file.imports_at(&db, after)).contains(&"cli".to_string()));
}

#[test]
fn test_imports_at_covers_the_same_layers_as_the_per_sourcing_file_view() {
    // Both views narrow to `offset` by the same rule and cover the same layers,
    // grouped differently. `imports_at` is band-major, every sourcing file's
    // `above` band, then own attaches, then every sourcing file's `below` band.
    // The per-sourcing-file view is context-major, one file's `above` / own /
    // `below` in full before the next file's. So the comparison below is over
    // layer sets, not order.
    //
    // Nothing in production reads `imports_at` today (`resolve_at` moved to the
    // per-sourcing-file view, completions will want the flat one), so this pins
    // the two together against drift in the narrowing.
    let mut db = TestDb::new();
    install_packages(&mut db, &["base", "dplyr", "rlang"]);
    let root = workspace_root(&db, "w");

    // `a.R` attaches a package and `b.R` doesn't, so the two contexts differ
    // and band-major genuinely reorders against context-major. `helpers.R`'s
    // own attach sits after the cursor, so an own-attach view that stopped
    // narrowing would show up as an extra `rlang` layer on one side.
    let a = make_file(&mut db, "w/a.R", "library(dplyr)\nsource(\"helpers.R\")\n");
    let b = make_file(&mut db, "w/b.R", "foo <- 1\nsource(\"helpers.R\")\n");
    let helpers = make_file(&mut db, "w/helpers.R", "foo\nlibrary(rlang)\n");
    root.set_scripts(&mut db).to(vec![a, b, helpers]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let offset = TextSize::from(0);
    let contexts = helpers.imports_by_sourcing_file_at(&db, offset);
    assert_eq!(contexts.len(), 2);

    let flat = helpers.imports_at(&db, offset);
    let grouped: Vec<ImportLayer> = contexts.into_iter().flatten().collect();
    assert_eq!(layer_keys(&db, &flat), layer_keys(&db, &grouped));
}

#[test]
fn test_attach_under_a_lazy_ancestor_stays_invisible() {
    // The `local()` here is eager, but it sits in a function body, and nothing
    // says that function was ever called. So the whole chain out to the file
    // scope has to be eager, not just the attach's own scope.
    let mut db = TestDb::new();
    install_packages(&mut db, &["cli"]);

    let source = "f <- function() {\n  local({\n    library(cli)\n  })\n}\nafter\n";
    let file = make_file(&mut db, "a.R", source);

    let after = TextSize::from(source.find("after").unwrap() as u32);
    assert!(!library_attaches(&db, &file.imports_at(&db, after)).contains(&"cli".to_string()));
}

#[test]
fn test_inherited_attaches_rank_by_source_position_across_source_calls() {
    // `pkgb` is visible only at the first `source()` call, but it attached after
    // `pkga`. The merged contexts must preserve that precedence.
    let mut db = TestDb::new();
    install_packages(&mut db, &["pkga", "pkgb"]);
    let root = workspace_root(&db, "w");

    let main = make_file(
        &mut db,
        "w/main.R",
        "library(pkga)\nif (dev) {\n  library(pkgb)\n  source(\"helpers.R\")\n}\nsource(\"helpers.R\")\n",
    );
    let helpers_source = "top <- 1\n";
    let helpers = make_file(&mut db, "w/helpers.R", helpers_source);

    root.set_scripts(&mut db).to(vec![main, helpers]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let offset = TextSize::from(helpers_source.find("top").unwrap() as u32);
    let attaches = library_attaches(&db, &helpers.imports_at(&db, offset));

    assert_eq!(attaches, vec!["pkgb".to_string(), "pkga".to_string()]);
}
