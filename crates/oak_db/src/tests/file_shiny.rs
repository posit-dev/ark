use salsa::Setter;

use crate::tests::file_imports::install_packages;
use crate::tests::file_imports::shape;
use crate::tests::test_db::file_path;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::DbInputs;
use crate::File;
use crate::FileRevision;
use crate::Package;

fn script_workspace(db: &mut TestDb, scripts: &[(&str, &str)]) -> (crate::Root, Vec<File>) {
    let root = workspace_root(db, "ws");
    let files: Vec<File> = scripts
        .iter()
        .map(|(path, contents)| {
            File::new(
                db,
                file_path(path),
                FileRevision::zero(),
                Some(contents.to_string()),
                None,
            )
        })
        .collect();
    root.set_scripts(db).to(files.clone());
    db.workspace_roots().set_roots(db).to(vec![root]);
    (root, files)
}

#[test]
fn test_shiny_entry_sees_global_then_the_autoloaded_directory() {
    let mut db = TestDb::new();
    install_packages(&mut db, &["base", "shiny"]);
    let (_, files) = script_workspace(&mut db, &[
        ("ws/app.R", "shinyApp(ui, server)\n"),
        ("ws/global.R", "cfg <- 1\n"),
        ("ws/R/a.R", "a_val <- 1\n"),
        ("ws/R/b.R", "b_val <- 2\n"),
    ]);

    // `R/` bindings shadow `global.R` because it loads first into their parent environment.
    assert_eq!(shape(&db, files[0].imports(&db)), vec![
        "File(b.R)".to_string(),
        "File(a.R)".to_string(),
        "File(global.R)".to_string(),
        "Package(shiny)".to_string(),
        "Package(base)".to_string(),
    ]);
}

#[test]
fn test_shiny_autoload_does_not_recurse() {
    // `loadSupport()` lists `R/` with `recursive=FALSE`, unlike `tar_source()`.
    let mut db = TestDb::new();
    let (_, files) = script_workspace(&mut db, &[
        ("ws/app.R", "shinyApp(ui, server)\n"),
        ("ws/R/a.R", "a_val <- 1\n"),
        ("ws/R/models/fit.R", "fit_val <- 2\n"),
    ]);

    assert_eq!(shape(&db, files[0].imports(&db)), vec![
        "File(a.R)".to_string()
    ]);
}

#[test]
fn test_shiny_ui_and_server_do_not_see_each_other() {
    // `ui.R` and `server.R` run in sibling environments, so neither imports the other.
    let mut db = TestDb::new();
    let (_, files) = script_workspace(&mut db, &[
        ("ws/ui.R", "shinyUI(fluidPage())\n"),
        ("ws/server.R", "shinyServer(function(input, output) NULL)\n"),
        ("ws/R/a.R", "a_val <- 1\n"),
    ]);

    assert_eq!(shape(&db, files[0].imports(&db)), vec![
        "File(a.R)".to_string()
    ]);
    assert_eq!(shape(&db, files[1].imports(&db)), vec![
        "File(a.R)".to_string()
    ]);
}

#[test]
fn test_shiny_disable_autoload_drops_the_directory_but_keeps_global() {
    // `_disable_autoload.R` prevents `R/` loading after `global.R` is evaluated.
    let mut db = TestDb::new();
    install_packages(&mut db, &["base", "shiny"]);
    let (_, files) = script_workspace(&mut db, &[
        ("ws/app.R", "shinyApp(ui, server)\n"),
        ("ws/global.R", "cfg <- 1\n"),
        ("ws/R/a.R", "a_val <- 1\n"),
        ("ws/R/_disable_autoload.R", "\n"),
    ]);
    let a = files[2];

    assert_eq!(shape(&db, files[0].imports(&db)), vec![
        "File(global.R)".to_string(),
        "Package(shiny)".to_string(),
        "Package(base)".to_string(),
    ]);

    // Disabled autoload leaves `a.R` on plain `R/` collation, so it inherits
    // neither `global.R` nor `shiny`.
    assert_eq!(shape(&db, a.imports(&db)), vec![
        "File(_disable_autoload.R)".to_string(),
        "Package(base)".to_string(),
    ]);
}

#[test]
fn test_confirmed_entry_point_with_nothing_to_inherit_still_attaches_shiny() {
    let mut db = TestDb::new();
    install_packages(&mut db, &["base", "shiny"]);
    let (_, files) = script_workspace(&mut db, &[("ws/app.R", "shinyApp(ui, server)\n")]);

    assert_eq!(shape(&db, files[0].imports(&db)), vec![
        "Package(shiny)".to_string(),
        "Package(base)".to_string(),
    ]);
}

#[test]
fn test_autoloaded_file_with_no_global_still_gets_the_implicit_shiny_attach() {
    let mut db = TestDb::new();
    install_packages(&mut db, &["base", "shiny"]);
    let (_, files) = script_workspace(&mut db, &[
        ("ws/app.R", "shinyApp(ui, server)\n"),
        ("ws/R/a.R", "a_val <- 1\n"),
        ("ws/R/b.R", "b_val <- 2\n"),
    ]);

    assert_eq!(shape(&db, files[2].imports(&db)), vec![
        "File(a.R)".to_string(),
        "Package(shiny)".to_string(),
        "Package(base)".to_string(),
    ]);
}

#[test]
fn test_autoloaded_file_sees_global_and_the_implicit_shiny_attach() {
    let mut db = TestDb::new();
    install_packages(&mut db, &["base", "dplyr", "shiny"]);
    let (_, files) = script_workspace(&mut db, &[
        ("ws/app.R", "shinyApp(ui, server)\n"),
        ("ws/global.R", "library(dplyr)\n"),
        ("ws/R/mod.R", "mod_val <- 1\n"),
        ("ws/R/util.R", "util_val <- 2\n"),
    ]);

    assert_eq!(shape(&db, files[2].imports(&db)), vec![
        "File(util.R)".to_string(),
        "File(global.R)".to_string(),
        "Package(dplyr)".to_string(),
        "Package(shiny)".to_string(),
        "Package(base)".to_string(),
    ]);
}

#[test]
fn test_r_directory_without_an_entry_point_ignores_global() {
    let mut db = TestDb::new();
    install_packages(&mut db, &["base", "shiny"]);
    let (_, files) = script_workspace(&mut db, &[
        ("ws/global.R", "cfg <- 1\n"),
        ("ws/R/a.R", "a_val <- 1\n"),
        ("ws/R/b.R", "b_val <- 2\n"),
    ]);

    assert_eq!(shape(&db, files[1].imports(&db)), vec![
        "File(b.R)".to_string(),
        "Package(base)".to_string(),
    ]);
}

#[test]
fn test_app_r_without_the_shiny_app_call_is_not_detected() {
    let mut db = TestDb::new();
    install_packages(&mut db, &["base", "shiny"]);
    let (_, files) = script_workspace(&mut db, &[
        ("ws/app.R", "1 + 1\n"),
        ("ws/global.R", "cfg <- 1\n"),
        ("ws/R/a.R", "a_val <- 1\n"),
    ]);

    assert_eq!(shape(&db, files[0].imports(&db)), vec![
        "Package(base)".to_string()
    ]);
}

#[test]
fn test_shiny_layers_backdate_on_unrelated_script_change() {
    // Adding a script outside the app must not rerun `cross_file_layers()` when
    // `shiny_autoload()` backdates the unchanged app-specific result.
    let mut db = TestDb::new();
    let (root, files) = script_workspace(&mut db, &[
        ("ws/app.R", "shinyApp(ui, server)\n"),
        ("ws/R/a.R", "a_val <- 1\n"),
    ]);
    let app = files[0];

    let _ = app.imports(&db);
    assert_eq!(db.executions("cross_file_layers"), 1);

    let elsewhere = File::new(
        &db,
        file_path("ws/other/z.R"),
        FileRevision::zero(),
        Some("z_val <- 1\n".to_string()),
        None,
    );
    let mut scripts = files.clone();
    scripts.push(elsewhere);
    root.set_scripts(&mut db).to(scripts);

    let _ = app.imports(&db);
    assert_eq!(db.executions("cross_file_layers"), 1);
}

#[test]
fn test_editing_entry_file_body_reexecutes_only_the_classifier() {
    // A source edit that preserves entry-point classification must backdate
    // `shiny_autoload()` and `cross_file_layers()`.
    let mut db = TestDb::new();
    let (_, files) = script_workspace(&mut db, &[
        ("ws/app.R", "shinyApp(ui, server)\n"),
        ("ws/R/a.R", "a_val <- 1\n"),
    ]);
    let (app, a) = (files[0], files[1]);

    let _ = app.imports(&db);
    let _ = a.imports(&db);
    let classifier_before = db.executions("is_shiny_entry_file");
    let autoload_before = db.executions("shiny_autoload");
    let cross_file_layers_before = db.executions("cross_file_layers");

    app.set_source_text_override(&mut db).to(Some(
        "# a harmless comment\nshinyApp(ui, server)\n".to_string(),
    ));

    let _ = app.imports(&db);
    let _ = a.imports(&db);

    assert!(db.executions("is_shiny_entry_file") > classifier_before);
    assert_eq!(db.executions("shiny_autoload"), autoload_before);
    assert_eq!(db.executions("cross_file_layers"), cross_file_layers_before);
}

#[test]
fn test_removing_the_shiny_app_call_drops_the_shiny_layers() {
    let mut db = TestDb::new();
    install_packages(&mut db, &["base", "shiny"]);
    let (_, files) = script_workspace(&mut db, &[
        ("ws/app.R", "shinyApp(ui, server)\n"),
        ("ws/R/a.R", "a_val <- 1\n"),
    ]);
    let (app, a) = (files[0], files[1]);

    assert_eq!(shape(&db, app.imports(&db)), vec![
        "File(a.R)".to_string(),
        "Package(shiny)".to_string(),
        "Package(base)".to_string(),
    ]);
    assert_eq!(shape(&db, a.imports(&db)), vec![
        "Package(shiny)".to_string(),
        "Package(base)".to_string(),
    ]);

    app.set_source_text_override(&mut db)
        .to(Some("1 + 1\n".to_string()));

    assert_eq!(shape(&db, app.imports(&db)), vec![
        "Package(base)".to_string()
    ]);
    assert_eq!(
        shape(&db, a.imports(&db)),
        vec!["Package(base)".to_string()]
    );
}

#[test]
fn test_adding_the_shiny_app_call_creates_the_shiny_layers() {
    let mut db = TestDb::new();
    install_packages(&mut db, &["base", "shiny"]);
    let (_, files) = script_workspace(&mut db, &[
        ("ws/app.R", "1 + 1\n"),
        ("ws/R/a.R", "a_val <- 1\n"),
    ]);
    let (app, a) = (files[0], files[1]);

    assert_eq!(shape(&db, app.imports(&db)), vec![
        "Package(base)".to_string()
    ]);
    assert_eq!(
        shape(&db, a.imports(&db)),
        vec!["Package(base)".to_string()]
    );

    app.set_source_text_override(&mut db)
        .to(Some("shinyApp(ui, server)\n".to_string()));

    assert_eq!(shape(&db, app.imports(&db)), vec![
        "File(a.R)".to_string(),
        "Package(shiny)".to_string(),
        "Package(base)".to_string(),
    ]);
    assert_eq!(shape(&db, a.imports(&db)), vec![
        "Package(shiny)".to_string(),
        "Package(base)".to_string(),
    ]);
}

#[test]
fn test_package_contained_shiny_app_is_governed_by_shiny() {
    // Package ownership does not suppress a Shiny app under `inst/app/` because
    // `package.files()` does not load it.
    let mut db = TestDb::new();
    install_packages(&mut db, &["base", "shiny"]);
    let workspace = workspace_root(&db, "w");
    let pkg = Package::new(
        &db,
        file_path("w/pkg/DESCRIPTION"),
        "pkg".to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        None,
        Vec::new(),
        Vec::new(),
    );
    let app = File::new(
        &db,
        file_path("w/pkg/inst/app/app.R"),
        FileRevision::zero(),
        Some("shinyApp(ui, server)\n".to_string()),
        Some(pkg),
    );
    let helper = File::new(
        &db,
        file_path("w/pkg/inst/app/R/mod.R"),
        FileRevision::zero(),
        Some("mod_val <- 1\n".to_string()),
        Some(pkg),
    );
    pkg.set_scripts(&mut db).to(vec![app, helper]);
    workspace.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![workspace]);

    assert_eq!(shape(&db, app.imports(&db)), vec![
        "File(mod.R)".to_string(),
        "Package(shiny)".to_string(),
        "Package(base)".to_string(),
    ]);
    assert_eq!(shape(&db, helper.imports(&db)), vec![
        "Package(shiny)".to_string(),
        "Package(base)".to_string(),
    ]);
}

#[test]
fn test_package_r_directory_is_not_a_shiny_support_directory() {
    let mut db = TestDb::new();
    install_packages(&mut db, &["base", "shiny"]);
    let workspace = workspace_root(&db, "w");
    let pkg = Package::new(
        &db,
        file_path("w/pkg/DESCRIPTION"),
        "pkg".to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        None,
        Vec::new(),
        Vec::new(),
    );
    let source = File::new(
        &db,
        file_path("w/pkg/R/util.R"),
        FileRevision::zero(),
        Some("util_val <- 1\n".to_string()),
        Some(pkg),
    );
    let app = File::new(
        &db,
        file_path("w/pkg/inst/app/app.R"),
        FileRevision::zero(),
        Some("shinyApp(ui, server)\n".to_string()),
        Some(pkg),
    );
    pkg.set_files(&mut db).to(vec![source]);
    pkg.set_scripts(&mut db).to(vec![app]);
    workspace.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![workspace]);

    assert_eq!(shape(&db, source.imports(&db)), vec![
        "Package(base)".to_string()
    ]);
}

#[test]
fn test_module_named_app_r_resolves_against_the_enclosing_app() {
    // An enclosing app keeps `R/app.R` in its support collation rather than
    // treating it as a separate app root.
    let mut db = TestDb::new();
    install_packages(&mut db, &["base", "shiny"]);
    let (_, files) = script_workspace(&mut db, &[
        ("ws/app.R", "shinyApp(ui, server)\n"),
        ("ws/global.R", "cfg <- 1\n"),
        ("ws/R/app.R", "app_val <- shinyApp(ui, server)\n"),
        ("ws/R/z.R", "z_val <- 2\n"),
    ]);

    assert_eq!(shape(&db, files[2].imports(&db)), vec![
        "File(z.R)".to_string(),
        "File(global.R)".to_string(),
        "Package(shiny)".to_string(),
        "Package(base)".to_string(),
    ]);
}

#[test]
fn test_app_rooted_at_an_r_directory_is_still_an_entry_point() {
    let mut db = TestDb::new();
    install_packages(&mut db, &["base", "shiny"]);
    let (_, files) = script_workspace(&mut db, &[
        ("ws/R/app.R", "shinyApp(ui, server)\n"),
        ("ws/R/util.R", "util_val <- 1\n"),
    ]);

    assert_eq!(shape(&db, files[0].imports(&db)), vec![
        "File(util.R)".to_string(),
        "Package(shiny)".to_string(),
        "Package(base)".to_string(),
    ]);
}
