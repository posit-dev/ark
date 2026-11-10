use std::path::Path;

use aether_path::FilePath;
use oak_core::string::decode_string_prefix;
use oak_db::DbInputs;
use oak_db::File;
use oak_db::OakDatabase;
use oak_db::Root;
use oak_db::RootKind;
use oak_ide::file_path_completions;
use oak_scan::DbScan;
use salsa::Setter;

/// Decode `token` up to its `@` cursor marker, or in full if there is no
/// marker. A malformed prefix gives no labels, matching the completion host.
fn labels(db: &OakDatabase, file: File, token: &str) -> Vec<String> {
    labels_with_fallback(db, file, token, None)
}

fn labels_with_fallback(
    db: &OakDatabase,
    file: File,
    token: &str,
    fallback_dir: Option<&Path>,
) -> Vec<String> {
    let prefix = match token.split_once('@') {
        Some((prefix, _)) => prefix,
        None => token,
    };
    let Some(typed_path) = decode_string_prefix(prefix) else {
        return Vec::new();
    };

    file_path_completions(db, file, &typed_path, fallback_dir)
        .unwrap()
        .into_iter()
        .map(|item| {
            if item.is_dir {
                format!("{}/", item.name)
            } else {
                item.name
            }
        })
        .collect()
}

fn file_path(path: &Path) -> FilePath {
    FilePath::from_path_buf(path.to_path_buf()).unwrap()
}

fn touch(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "").unwrap();
}

/// A workspace `proj/` with `R/top.R`, `sub/R/sub.R` and `sub/data/`.
/// Returns the root and the editor buffer `sub/script.R`.
fn subproject(db: &mut OakDatabase, dir: &Path) -> (Root, File) {
    let proj = dir.join("proj");
    touch(&proj.join("R/top.R"));
    touch(&proj.join("sub/R/sub.R"));
    std::fs::create_dir_all(proj.join("sub/data")).unwrap();

    let root = Root::new(
        db,
        file_path(&proj),
        RootKind::Workspace,
        vec![],
        vec![],
        vec![],
    );
    db.workspace_roots().set_roots(db).to(vec![root]);

    let script = db.upsert_editor(file_path(&proj.join("sub/script.R")), String::new());
    (root, script)
}

#[test]
fn test_relative_to_workspace_root() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();
    let (_, script) = subproject(&mut db, dir.path());

    assert_eq!(labels(&db, script, r#""R/""#), vec!["top.R"]);
    assert_eq!(labels(&db, script, r#""""#), vec!["R/", "sub/"]);
}

#[test]
fn test_relative_to_nested_environment_dir() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();
    let (root, script) = subproject(&mut db, dir.path());
    root.set_environment_dirs(&mut db)
        .to(vec![file_path(&dir.path().join("proj/sub"))]);

    assert_eq!(labels(&db, script, r#""R/""#), vec!["sub.R"]);
    assert_eq!(labels(&db, script, r#""""#), vec!["R/", "data/"]);
    assert_eq!(labels(&db, script, r#""../R/""#), vec!["top.R"]);
}

#[test]
fn test_partial_name_lists_parent() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();
    let (_, script) = subproject(&mut db, dir.path());

    assert_eq!(labels(&db, script, r#""R/to""#), vec!["top.R"]);
    assert_eq!(labels(&db, script, r#""su""#), vec!["R/", "sub/"]);
}

#[test]
fn test_escapes_are_decoded() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();
    let (_, script) = subproject(&mut db, dir.path());

    assert_eq!(labels(&db, script, r#""\x52/""#), vec!["top.R"]);
    assert_eq!(labels(&db, script, r#"'sub\u002fR/'"#), vec!["sub.R"]);
    assert_eq!(labels(&db, script, r#"r"(R/)""#), vec!["top.R"]);
}

#[test]
fn test_unclosed_string() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();
    let (_, script) = subproject(&mut db, dir.path());

    assert_eq!(labels(&db, script, r#""R/"#), vec!["top.R"]);
    assert_eq!(labels(&db, script, r#""su"#), vec!["R/", "sub/"]);
    assert_eq!(labels(&db, script, r#"""#), vec!["R/", "sub/"]);
}

#[test]
fn test_text_after_cursor_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();
    let (_, script) = subproject(&mut db, dir.path());

    // Decoding text after the cursor would select `sub/R/` instead of the root.
    assert_eq!(labels(&db, script, r#""@sub/R/""#), vec!["R/", "sub/"]);
    assert_eq!(labels(&db, script, r#""R/@sub/R/""#), vec!["top.R"]);
}

#[test]
fn test_trailing_backslash() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();
    let (_, script) = subproject(&mut db, dir.path());

    // Omitting the unfinished escape preserves the directory prefix `R/`.
    assert_eq!(labels(&db, script, r#""R/\@""#), vec!["top.R"]);
}

#[test]
fn test_unclosed_raw_string() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();
    let (_, script) = subproject(&mut db, dir.path());

    assert_eq!(labels(&db, script, r#"r"(R/"#), vec!["top.R"]);
    assert_eq!(labels(&db, script, r#"r"(sub/@R)""#), vec!["R/", "data/"]);
    assert_eq!(labels(&db, script, r#"r"-[@"#), vec!["R/", "sub/"]);
}

#[test]
fn test_invalid_escape_lists_nothing() {
    // https://github.com/posit-dev/positron/issues/6584
    let dir = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();
    let (_, script) = subproject(&mut db, dir.path());

    assert_eq!(labels(&db, script, r#"".\R\top.R""#), Vec::<String>::new());
}

#[test]
fn test_url_lists_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();
    let (_, script) = subproject(&mut db, dir.path());

    assert_eq!(
        labels(&db, script, "'https://posit.co/x'"),
        Vec::<String>::new()
    );
}

#[test]
fn test_absolute_path() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();
    let (_, script) = subproject(&mut db, dir.path());

    // R accepts forward slashes on Windows as well as Unix.
    let path = dir
        .path()
        .join("proj")
        .join("R")
        .to_string_lossy()
        .replace('\\', "/");
    assert_eq!(labels(&db, script, &format!("'{path}/'")), vec!["top.R"]);
}

#[test]
fn test_home_directory() {
    // Nextest isolates these environment changes in a separate test process.
    let home = tempfile::tempdir().unwrap();
    touch(&home.path().join("analysis/main.R"));
    std::env::set_var("HOME", home.path());
    std::env::set_var("R_USER", home.path());

    let mut db = OakDatabase::new();
    let script = db.upsert_editor(
        FilePath::parse("untitled:Untitled-1").unwrap(),
        String::new(),
    );

    assert_eq!(labels(&db, script, "'~/'"), vec!["analysis/"]);
    assert_eq!(labels(&db, script, "'~'"), vec!["analysis/"]);
    assert_eq!(labels(&db, script, "'~/analysis/'"), vec!["main.R"]);
    assert_eq!(labels(&db, script, "'~/an'"), vec!["analysis/"]);
    assert_eq!(labels(&db, script, "'~//'"), vec!["analysis/"]);
    assert_eq!(labels(&db, script, "'~//analysis/'"), vec!["main.R"]);
}

#[test]
fn test_file_outside_workspace_uses_its_directory() {
    let dir = tempfile::tempdir().unwrap();
    touch(&dir.path().join("scratch/R/a.R"));

    let mut db = OakDatabase::new();
    let script = db.upsert_editor(
        file_path(&dir.path().join("scratch/script.R")),
        String::new(),
    );

    assert_eq!(labels(&db, script, r#""R/""#), vec!["a.R"]);
}

#[test]
fn test_document_without_anchor_uses_fallback_dir() {
    let dir = tempfile::tempdir().unwrap();
    touch(&dir.path().join("R/a.R"));

    let mut db = OakDatabase::new();
    let untitled = db.upsert_editor(
        FilePath::parse("untitled:Untitled-1").unwrap(),
        String::new(),
    );

    assert_eq!(
        labels_with_fallback(&db, untitled, r#""R/""#, Some(dir.path())),
        vec!["a.R"]
    );
}

#[test]
fn test_anchor_takes_precedence_over_fallback_dir() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();
    let (_, script) = subproject(&mut db, dir.path());

    let elsewhere = tempfile::tempdir().unwrap();
    touch(&elsewhere.path().join("R/other.R"));

    assert_eq!(
        labels_with_fallback(&db, script, r#""R/""#, Some(elsewhere.path())),
        vec!["top.R"]
    );
}

#[test]
fn test_document_without_anchor_or_fallback_dir() {
    let dir = tempfile::tempdir().unwrap();
    touch(&dir.path().join("R/a.R"));

    let mut db = OakDatabase::new();
    let untitled = db.upsert_editor(
        FilePath::parse("untitled:Untitled-1").unwrap(),
        String::new(),
    );

    assert_eq!(labels(&db, untitled, r#""R/""#), Vec::<String>::new());
    let path = dir.path().join("R").to_string_lossy().replace('\\', "/");
    assert_eq!(labels(&db, untitled, &format!("'{path}/'")), vec!["a.R"]);
}
