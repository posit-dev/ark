use aether_parser::parse;
use aether_parser::RParserOptions;
use oak_semantic::build_index;
use oak_semantic::effects;
use oak_semantic::effects::DirWalk;
use oak_semantic::semantic_index::SemanticCallKind;
use oak_semantic::semantic_index::SemanticIndex;
use oak_semantic::EffectsHandlers;
use oak_semantic::ImportsResolver;
use oak_semantic::SourceResolution;
use url::Url;

use crate::common::semantic_call_kinds;

/// Resolves `tar_source` against the targets registry entry, and stands in for
/// the workspace listing: `files` are what a directory expands to, `file` is
/// what a path resolving to a script gives.
struct TargetsResolver {
    file: Option<SourceResolution>,
    files: Vec<SourceResolution>,
}

impl TargetsResolver {
    fn with_dir(files: Vec<SourceResolution>) -> Self {
        Self { file: None, files }
    }
}

impl ImportsResolver for TargetsResolver {
    fn resolve_source(&mut self, _path: &str) -> Option<SourceResolution> {
        self.file.clone()
    }

    fn resolve_source_dir(&mut self, _path: &str, walk: DirWalk) -> Vec<SourceResolution> {
        // `tar_source()` lists directory arguments with `recursive = TRUE`.
        assert_eq!(walk, DirWalk::Recursive);
        self.files.clone()
    }

    fn resolve_effects(&mut self, name: &str, _: &[String]) -> Option<EffectsHandlers> {
        effects::lookup("targets", name)
            .or_else(|| effects::lookup("base", name))
            .copied()
    }
}

fn index(source: &str, resolver: TargetsResolver) -> SemanticIndex {
    let parsed = parse(source, RParserOptions::default());
    if parsed.has_error() {
        panic!("source has syntax errors: {source}");
    }
    build_index(&parsed.tree(), resolver)
}

fn resolution(url: &str, name: &str) -> SourceResolution {
    SourceResolution {
        url: Url::parse(url).unwrap(),
        names: vec![name.to_string()],
        packages: vec![],
    }
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
    let files = vec![
        resolution("file:///R/a.R", "a_name"),
        resolution("file:///R/b.R", "b_name"),
    ];
    let index = index("tar_source()\n", TargetsResolver::with_dir(files));

    assert_eq!(semantic_call_kinds(&index), [
        &sourced("R", "file:///R/a.R"),
        &sourced("R", "file:///R/b.R"),
    ]);
}

#[test]
fn test_tar_source_positional_directory() {
    let files = vec![resolution("file:///code/a.R", "a_name")];
    let index = index("tar_source(\"code\")\n", TargetsResolver::with_dir(files));

    assert_eq!(semantic_call_kinds(&index), [&sourced(
        "code",
        "file:///code/a.R"
    )]);
}

#[test]
fn test_tar_source_qualified_call_is_recognized() {
    let files = vec![resolution("file:///R/a.R", "a_name")];
    let index = index("targets::tar_source()\n", TargetsResolver::with_dir(files));

    assert_eq!(semantic_call_kinds(&index), [&sourced(
        "R",
        "file:///R/a.R"
    )]);
}

#[test]
fn test_tar_source_path_naming_a_script_resolves_as_a_file() {
    // `files` takes scripts as well as directories, so a `FileOrDir` target
    // tries the file first and only falls back to a listing.
    let resolver = TargetsResolver {
        file: Some(resolution("file:///R/utils.R", "util")),
        files: vec![resolution("file:///unused.R", "unused")],
    };
    let index = index("tar_source(\"R/utils.R\")\n", resolver);

    assert_eq!(semantic_call_kinds(&index), [&sourced(
        "R/utils.R",
        "file:///R/utils.R"
    )]);
}

#[test]
fn test_tar_source_named_files_argument_is_recognized() {
    let files = vec![resolution("file:///code/a.R", "a_name")];
    let index = index(
        "tar_source(files = \"code\")\n",
        TargetsResolver::with_dir(files),
    );

    assert_eq!(semantic_call_kinds(&index), [&sourced(
        "code",
        "file:///code/a.R"
    )]);
}

#[test]
fn test_tar_source_change_directory_false_uses_the_default_directory() {
    // `change_directory` does not bind `files`, so `files` uses its `"R"` default.
    let files = vec![resolution("file:///R/a.R", "a_name")];
    let index = index(
        "tar_source(change_directory = FALSE)\n",
        TargetsResolver::with_dir(files),
    );

    assert_eq!(semantic_call_kinds(&index), [&sourced(
        "R",
        "file:///R/a.R"
    )]);
}

#[test]
fn test_tar_source_dynamic_files_argument_is_not_recognized() {
    // A dynamic `files` value overrides the `"R"` default but produces no source call.
    let files = vec![resolution("file:///R/a.R", "a_name")];
    let index = index(
        "tar_source(files = some_var)\n",
        TargetsResolver::with_dir(files),
    );

    assert_eq!(semantic_call_kinds(&index), Vec::<&SemanticCallKind>::new());
}
