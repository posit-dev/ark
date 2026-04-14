use std::collections::HashMap;
use std::path::PathBuf;

use aether_parser::parse;
use aether_parser::RParserOptions;
use assert_matches::assert_matches;
use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_ide::goto_definition;
use oak_ide::ResolvedDefinition;
use oak_index::external::file_layers;
use oak_index::external::BindingSource;
use oak_index::semantic_index;
use oak_package::library::Library;
use oak_package::package::Package;
use oak_package::package_description::Description;
use oak_package::package_namespace::Namespace;
use url::Url;

fn index(source: &str) -> oak_index::semantic_index::SemanticIndex {
    let parsed = parse(source, RParserOptions::default());
    semantic_index(&parsed.tree())
}

fn empty_library() -> Library {
    Library::new(vec![])
}

fn test_library(packages: Vec<(&str, Vec<&str>)>) -> Library {
    let mut library = Library::new(vec![]);
    for (name, exports) in packages {
        let ns = Namespace {
            exports: exports.into_iter().map(String::from).collect(),
            ..Default::default()
        };
        let desc = Description {
            name: name.to_string(),
            ..Default::default()
        };
        let pkg = Package::from_parts(PathBuf::from("/fake"), desc, ns);
        library = library.insert(name, pkg);
    }
    library
}

fn text_range(start: u32, end: u32) -> TextRange {
    TextRange::new(TextSize::from(start), TextSize::from(end))
}

fn file_url(name: &str) -> Url {
    Url::parse(&format!("file:///project/R/{name}")).unwrap()
}

fn offset(n: u32) -> TextSize {
    TextSize::from(n)
}

// --- Local resolution ---

#[test]
fn test_local_simple() {
    // "x <- 1\nx\n"
    //  0123456 78
    let source = "x <- 1\nx\n";
    let idx = index(source);
    let library = empty_library();

    let result = goto_definition(&idx, &[], &library, offset(7));
    assert_matches!(result, Some(ResolvedDefinition::Local { range }) => {
        assert_eq!(range, text_range(0, 1));
    });
}

#[test]
fn test_local_multiple_definitions() {
    // First def wins for goto-definition
    let source = "x <- 1\nx <- 2\nx\n";
    let idx = index(source);
    let library = empty_library();

    let result = goto_definition(&idx, &[], &library, offset(14));
    assert_matches!(result, Some(ResolvedDefinition::Local { .. }));
}

#[test]
fn test_local_in_function() {
    let source = "f <- function() {\n  x <- 1\n  x\n}\n";
    let idx = index(source);
    let library = empty_library();

    // `x` use is inside the function body. Find its offset.
    let use_offset = source.rfind('x').unwrap() as u32;
    let result = goto_definition(&idx, &[], &library, offset(use_offset));
    assert_matches!(result, Some(ResolvedDefinition::Local { .. }));
}

#[test]
fn test_local_parameter() {
    let source = "f <- function(x) {\n  x\n}\n";
    let idx = index(source);
    let library = empty_library();

    let use_offset = source.rfind('x').unwrap() as u32;
    let result = goto_definition(&idx, &[], &library, offset(use_offset));
    assert_matches!(result, Some(ResolvedDefinition::Local { .. }));
}

// --- Enclosing scope resolution ---

#[test]
fn test_enclosing_scope() {
    let source = "x <- 1\nf <- function() {\n  x\n}\n";
    let idx = index(source);
    let library = empty_library();

    // `x` in the function body should resolve to the enclosing definition
    let use_offset = source.rfind('x').unwrap() as u32;
    let result = goto_definition(&idx, &[], &library, offset(use_offset));
    assert_matches!(result, Some(ResolvedDefinition::Local { range }) => {
        assert_eq!(range, text_range(0, 1));
    });
}

// --- External resolution: project file ---

#[test]
fn test_external_project_file() {
    // Source file only has a use of `foo`, no definition
    let source = "foo\n";
    let idx = index(source);
    let library = empty_library();

    // Build a scope chain with a predecessor file that exports `foo`
    let other_url = file_url("other.R");
    let other_source = "foo <- 42\n";
    let other_idx = index(other_source);
    let scope_chain = file_layers(other_url.clone(), &other_idx);

    let result = goto_definition(&idx, &scope_chain, &library, offset(0));
    assert_matches!(result, Some(ResolvedDefinition::ProjectFile { file, name, range }) => {
        assert_eq!(file, other_url);
        assert_eq!(name, "foo");
        assert_eq!(range, text_range(0, 3));
    });
}

// --- External resolution: package ---

#[test]
fn test_external_package() {
    let source = "mutate\n";
    let idx = index(source);
    let library = test_library(vec![("dplyr", vec!["filter", "mutate", "select"])]);

    let scope_chain = vec![BindingSource::PackageExports("dplyr".to_string())];

    let result = goto_definition(&idx, &scope_chain, &library, offset(0));
    assert_matches!(result, Some(ResolvedDefinition::Package { package, name }) => {
        assert_eq!(package, "dplyr");
        assert_eq!(name, "mutate");
    });
}

// --- External resolution: importFrom ---

#[test]
fn test_external_import_from() {
    let source = "tibble\n";
    let idx = index(source);
    let library = empty_library();

    let mut imports = HashMap::new();
    imports.insert("tibble".to_string(), "tibble".to_string());
    let scope_chain = vec![BindingSource::PackageImports(imports)];

    let result = goto_definition(&idx, &scope_chain, &library, offset(0));
    assert_matches!(result, Some(ResolvedDefinition::Package { package, name }) => {
        assert_eq!(package, "tibble");
        assert_eq!(name, "tibble");
    });
}

// --- No resolution ---

#[test]
fn test_no_use_at_offset() {
    let source = "x <- 1\n";
    let idx = index(source);
    let library = empty_library();

    // Offset in whitespace / operator area
    let result = goto_definition(&idx, &[], &library, offset(3));
    assert_eq!(result, None);
}

#[test]
fn test_unresolved_symbol() {
    let source = "foo\n";
    let idx = index(source);
    let library = empty_library();

    // No scope chain, so external resolution fails too
    let result = goto_definition(&idx, &[], &library, offset(0));
    assert_eq!(result, None);
}

// --- Local takes precedence over external ---

#[test]
fn test_local_shadows_external() {
    let source = "foo <- 1\nfoo\n";
    let idx = index(source);
    let library = test_library(vec![("pkg", vec!["foo"])]);

    let scope_chain = vec![BindingSource::PackageExports("pkg".to_string())];

    // `foo` use at offset 9
    let use_offset = source.rfind("foo").unwrap() as u32;
    let result = goto_definition(&idx, &scope_chain, &library, offset(use_offset));
    assert_matches!(result, Some(ResolvedDefinition::Local { range }) => {
        assert_eq!(range, text_range(0, 3));
    });
}

// --- Conditional definition (may_be_unbound but has local defs) ---

#[test]
fn test_conditional_definition_prefers_local() {
    let source = "if (TRUE) x <- 1\nx\n";
    let idx = index(source);
    let library = test_library(vec![("pkg", vec!["x"])]);

    let scope_chain = vec![BindingSource::PackageExports("pkg".to_string())];

    let use_offset = source.rfind('x').unwrap() as u32;
    let result = goto_definition(&idx, &scope_chain, &library, offset(use_offset));
    // Even though `x` may be unbound, the local conditional def is preferred
    assert_matches!(result, Some(ResolvedDefinition::Local { .. }));
}
