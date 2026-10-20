use oak_semantic::effects::DirWalk;
use oak_semantic::semantic_index::SemanticCallKind;
use url::Url;

use crate::common::build_with;
use crate::common::semantic_call_kinds;
use crate::resolvers::TestImportsResolver;

/// Resolver with targets on the search path. `tar_source()` lists directory
/// arguments with `recursive = TRUE`, so directories are registered with
/// `DirWalk::Recursive` and a handler asking for a shallow listing finds none.
fn targets_resolver() -> TestImportsResolver {
    TestImportsResolver::with_attached(&["targets"])
}

fn sourced(path: &str, url: &str) -> SemanticCallKind {
    SemanticCallKind::Source {
        path: path.to_string(),
        resolved: Some(Url::parse(url).unwrap()),
    }
}

#[test]
fn test_tar_source_no_arguments_uses_the_default_directory() {
    // The bare `tar_source()` that most `_targets.R` pipelines write relies on
    // `files = "R"`, so the default has to stand in for an absent argument.
    let resolver = targets_resolver().with_source_dir("R", DirWalk::Recursive, &[
        ("R/a.R", &["a_name"]),
        ("R/b.R", &["b_name"]),
    ]);
    let index = build_with("tar_source()\n", resolver);

    assert_eq!(semantic_call_kinds(&index), [
        &sourced("R", "file:///R/a.R"),
        &sourced("R", "file:///R/b.R"),
    ]);
}

#[test]
fn test_tar_source_positional_directory() {
    let resolver = targets_resolver()
        .with_source_dir("code", DirWalk::Recursive, &[("code/a.R", &["a_name"])]);
    let index = build_with("tar_source(\"code\")\n", resolver);

    assert_eq!(semantic_call_kinds(&index), [&sourced(
        "code",
        "file:///code/a.R"
    )]);
}

#[test]
fn test_tar_source_qualified_call_is_recognized() {
    // targets is not attached, so only the `targets::` qualifier can resolve the callee.
    let resolver = TestImportsResolver::with_base()
        .with_source_dir("R", DirWalk::Recursive, &[("R/a.R", &["a_name"])]);
    let index = build_with("targets::tar_source()\n", resolver);

    assert_eq!(semantic_call_kinds(&index), [&sourced(
        "R",
        "file:///R/a.R"
    )]);
}

#[test]
fn test_tar_source_path_naming_a_script_resolves_as_a_file() {
    // `files` takes scripts as well as directories, so a `FileOrDir` target
    // tries the file first and only falls back to a listing. The decoy
    // directory entry must not be reached.
    let resolver = targets_resolver()
        .with_source("R/utils.R", &["util"])
        .with_source_dir("R/utils.R", DirWalk::Recursive, &[("unused.R", &[
            "unused",
        ])]);
    let index = build_with("tar_source(\"R/utils.R\")\n", resolver);

    assert_eq!(semantic_call_kinds(&index), [&sourced(
        "R/utils.R",
        "file:///R/utils.R"
    )]);
}

#[test]
fn test_tar_source_named_files_argument_is_recognized() {
    let resolver = targets_resolver()
        .with_source_dir("code", DirWalk::Recursive, &[("code/a.R", &["a_name"])]);
    let index = build_with("tar_source(files = \"code\")\n", resolver);

    assert_eq!(semantic_call_kinds(&index), [&sourced(
        "code",
        "file:///code/a.R"
    )]);
}

#[test]
fn test_tar_source_change_directory_false_uses_the_default_directory() {
    // `change_directory` does not bind `files`, so `files` uses its `"R"` default.
    let resolver =
        targets_resolver().with_source_dir("R", DirWalk::Recursive, &[("R/a.R", &["a_name"])]);
    let index = build_with("tar_source(change_directory = FALSE)\n", resolver);

    assert_eq!(semantic_call_kinds(&index), [&sourced(
        "R",
        "file:///R/a.R"
    )]);
}

#[test]
fn test_tar_source_c_of_script_and_directory() {
    let resolver = targets_resolver()
        .with_source("packages.R", &["packages"])
        .with_source_dir("R", DirWalk::Recursive, &[
            ("R/a.R", &["a_name"]),
            ("R/b.R", &["b_name"]),
        ]);
    let index = build_with("tar_source(c(\"packages.R\", \"R\"))\n", resolver);

    assert_eq!(semantic_call_kinds(&index), [
        &sourced("packages.R", "file:///packages.R"),
        &sourced("R", "file:///R/a.R"),
        &sourced("R", "file:///R/b.R"),
    ]);
}

#[test]
fn test_tar_source_c_with_dynamic_element_is_not_recognized() {
    let resolver =
        targets_resolver().with_source_dir("R", DirWalk::Recursive, &[("R/a.R", &["a_name"])]);
    let index = build_with("tar_source(c(\"R\", other_dir))\n", resolver);

    assert_eq!(semantic_call_kinds(&index), Vec::<&SemanticCallKind>::new());
}

#[test]
fn test_tar_source_dynamic_files_argument_is_not_recognized() {
    // A dynamic `files` value overrides the `"R"` default but produces no source call.
    let resolver =
        targets_resolver().with_source_dir("R", DirWalk::Recursive, &[("R/a.R", &["a_name"])]);
    let index = build_with("tar_source(files = some_var)\n", resolver);

    assert_eq!(semantic_call_kinds(&index), Vec::<&SemanticCallKind>::new());
}
