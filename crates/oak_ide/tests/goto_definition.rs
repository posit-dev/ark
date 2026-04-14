use std::collections::HashMap;
use std::path::PathBuf;

use aether_parser::parse;
use aether_parser::RParserOptions;
use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_ide::goto_definition;
use oak_ide::NavigationTarget;
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
    let file = file_url("test.R");
    let idx = index(source);
    let library = empty_library();

    let targets = goto_definition(&file, &idx, &[], &library, offset(7));
    assert_eq!(targets, vec![NavigationTarget {
        file,
        name: "x".to_string(),
        full_range: text_range(0, 1),
        focus_range: text_range(0, 1),
    }]);
}

#[test]
fn test_local_reassignment_shadows() {
    // Straight-line reassignment: second def kills the first
    let source = "x <- 1\nx <- 2\nx\n";
    let file = file_url("test.R");
    let idx = index(source);
    let library = empty_library();

    let targets = goto_definition(&file, &idx, &[], &library, offset(14));
    assert_eq!(targets, vec![NavigationTarget {
        file,
        name: "x".to_string(),
        full_range: text_range(7, 8),
        focus_range: text_range(7, 8),
    }]);
}

#[test]
fn test_local_conditional_returns_both() {
    let source = "if (TRUE) x <- 1 else x <- 2\nx\n";
    let file = file_url("test.R");
    let idx = index(source);
    let library = empty_library();

    let use_offset = source.rfind('x').unwrap() as u32;

    let targets = goto_definition(&file, &idx, &[], &library, offset(use_offset));
    assert_eq!(targets, vec![
        NavigationTarget {
            file: file.clone(),
            name: "x".to_string(),
            full_range: text_range(10, 11),
            focus_range: text_range(10, 11),
        },
        NavigationTarget {
            file,
            name: "x".to_string(),
            full_range: text_range(22, 23),
            focus_range: text_range(22, 23),
        },
    ]);
}

#[test]
fn test_local_in_function() {
    let source = "f <- function() {\n  x <- 1\n  x\n}\n";
    let file = file_url("test.R");
    let idx = index(source);
    let library = empty_library();

    let use_offset = source.rfind('x').unwrap() as u32;
    let targets = goto_definition(&file, &idx, &[], &library, offset(use_offset));
    assert_eq!(targets, vec![NavigationTarget {
        file,
        name: "x".to_string(),
        full_range: text_range(20, 21),
        focus_range: text_range(20, 21),
    }]);
}

#[test]
fn test_local_parameter() {
    let source = "f <- function(x) {\n  x\n}\n";
    let file = file_url("test.R");
    let idx = index(source);
    let library = empty_library();

    let use_offset = source.rfind('x').unwrap() as u32;
    let targets = goto_definition(&file, &idx, &[], &library, offset(use_offset));
    assert_eq!(targets, vec![NavigationTarget {
        file,
        name: "x".to_string(),
        full_range: text_range(14, 15),
        focus_range: text_range(14, 15),
    }]);
}

// --- Enclosing scope resolution ---

#[test]
fn test_enclosing_scope() {
    let source = "x <- 1\nf <- function() {\n  x\n}\n";
    let file = file_url("test.R");
    let idx = index(source);
    let library = empty_library();

    let use_offset = source.rfind('x').unwrap() as u32;
    let targets = goto_definition(&file, &idx, &[], &library, offset(use_offset));
    assert_eq!(targets, vec![NavigationTarget {
        file,
        name: "x".to_string(),
        full_range: text_range(0, 1),
        focus_range: text_range(0, 1),
    }]);
}

// --- External resolution: project file ---

#[test]
fn test_external_project_file() {
    let source = "foo\n";
    let file = file_url("current.R");
    let idx = index(source);
    let library = empty_library();

    let other_url = file_url("other.R");
    let other_source = "foo <- 42\n";
    let other_idx = index(other_source);
    let scope_chain = file_layers(other_url.clone(), &other_idx);

    let targets = goto_definition(&file, &idx, &scope_chain, &library, offset(0));
    assert_eq!(targets, vec![NavigationTarget {
        file: other_url,
        name: "foo".to_string(),
        full_range: text_range(0, 3),
        focus_range: text_range(0, 3),
    }]);
}

// --- External resolution: package ---

#[test]
fn test_external_package() {
    let source = "mutate\n";
    let file = file_url("test.R");
    let idx = index(source);
    let library = test_library(vec![("dplyr", vec!["filter", "mutate", "select"])]);

    let scope_chain = vec![BindingSource::PackageExports("dplyr".to_string())];

    let targets = goto_definition(&file, &idx, &scope_chain, &library, offset(0));
    // No navigation target for package symbols (no file/range to navigate to)
    assert!(targets.is_empty());
}

// --- External resolution: importFrom ---

#[test]
fn test_external_import_from() {
    let source = "tibble\n";
    let file = file_url("test.R");
    let idx = index(source);
    let library = empty_library();

    let mut imports = HashMap::new();
    imports.insert("tibble".to_string(), "tibble".to_string());
    let scope_chain = vec![BindingSource::PackageImports(imports)];

    let targets = goto_definition(&file, &idx, &scope_chain, &library, offset(0));
    // importFrom resolves to a package, no file/range to navigate to
    assert!(targets.is_empty());
}

// --- No resolution ---

#[test]
fn test_no_use_at_offset() {
    let source = "x <- 1\n";
    let file = file_url("test.R");
    let idx = index(source);
    let library = empty_library();

    let targets = goto_definition(&file, &idx, &[], &library, offset(3));
    assert!(targets.is_empty());
}

#[test]
fn test_unresolved_symbol() {
    let source = "foo\n";
    let file = file_url("test.R");
    let idx = index(source);
    let library = empty_library();

    let targets = goto_definition(&file, &idx, &[], &library, offset(0));
    assert!(targets.is_empty());
}

// --- Local takes precedence over external ---

#[test]
fn test_local_shadows_external() {
    let source = "foo <- 1\nfoo\n";
    let file = file_url("test.R");
    let idx = index(source);
    let library = test_library(vec![("pkg", vec!["foo"])]);

    let scope_chain = vec![BindingSource::PackageExports("pkg".to_string())];

    let use_offset = source.rfind("foo").unwrap() as u32;
    let targets = goto_definition(&file, &idx, &scope_chain, &library, offset(use_offset));
    assert_eq!(targets, vec![NavigationTarget {
        file,
        name: "foo".to_string(),
        full_range: text_range(0, 3),
        focus_range: text_range(0, 3),
    }]);
}

// --- Conditional definition (may_be_unbound but has local defs) ---

#[test]
fn test_conditional_definition_prefers_local() {
    let source = "if (TRUE) x <- 1\nx\n";
    let file = file_url("test.R");
    let idx = index(source);
    let library = test_library(vec![("pkg", vec!["x"])]);

    let scope_chain = vec![BindingSource::PackageExports("pkg".to_string())];

    let use_offset = source.rfind('x').unwrap() as u32;
    let targets = goto_definition(&file, &idx, &scope_chain, &library, offset(use_offset));
    assert_eq!(targets, vec![NavigationTarget {
        file,
        name: "x".to_string(),
        full_range: text_range(10, 11),
        focus_range: text_range(10, 11),
    }]);
}
