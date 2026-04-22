use std::collections::HashMap;
use std::path::PathBuf;

use aether_parser::parse;
use aether_parser::RParserOptions;
use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_ide::goto_definition;
use oak_ide::ExternalScope;
use oak_ide::NavigationTarget;
use oak_index::external::file_layers;
use oak_index::external::ScopeLayer;
use oak_index::semantic_index;
use oak_index::semantic_index::SemanticIndex;
use oak_package::library::Library;
use oak_package::package::Package;
use oak_package::package_description::Description;
use oak_package::package_namespace::Namespace;
use url::Url;

fn parse_source(source: &str) -> SemanticIndex {
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
    let idx = parse_source(source);
    let library = empty_library();

    let targets = goto_definition(offset(7), &file, &idx, &ExternalScope::default(), &library);
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
    let idx = parse_source(source);
    let library = empty_library();

    let targets = goto_definition(offset(14), &file, &idx, &ExternalScope::default(), &library);
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
    let idx = parse_source(source);
    let library = empty_library();

    let use_offset = source.rfind('x').unwrap() as u32;

    let targets = goto_definition(
        offset(use_offset),
        &file,
        &idx,
        &ExternalScope::default(),
        &library,
    );
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
    let idx = parse_source(source);
    let library = empty_library();

    let use_offset = source.rfind('x').unwrap() as u32;
    let targets = goto_definition(
        offset(use_offset),
        &file,
        &idx,
        &ExternalScope::default(),
        &library,
    );
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
    let idx = parse_source(source);
    let library = empty_library();

    let use_offset = source.rfind('x').unwrap() as u32;
    let targets = goto_definition(
        offset(use_offset),
        &file,
        &idx,
        &ExternalScope::default(),
        &library,
    );
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
    let idx = parse_source(source);
    let library = empty_library();

    let use_offset = source.rfind('x').unwrap() as u32;
    let targets = goto_definition(
        offset(use_offset),
        &file,
        &idx,
        &ExternalScope::default(),
        &library,
    );
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
    let idx = parse_source(source);
    let library = empty_library();

    let other_url = file_url("other.R");
    let other_source = "foo <- 42\n";
    let other_idx = parse_source(other_source);
    let scope_chain = file_layers(other_url.clone(), &other_idx);

    let targets = goto_definition(
        offset(0),
        &file,
        &idx,
        &ExternalScope::package(scope_chain.clone(), scope_chain),
        &library,
    );
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
    let idx = parse_source(source);
    let library = test_library(vec![("dplyr", vec!["filter", "mutate", "select"])]);

    let scope_chain = vec![ScopeLayer::PackageExports("dplyr".to_string())];

    let targets = goto_definition(
        offset(0),
        &file,
        &idx,
        &ExternalScope::package(scope_chain.clone(), scope_chain),
        &library,
    );
    // No navigation target for package symbols (no file/range to navigate to)
    assert!(targets.is_empty());
}

// --- External resolution: importFrom ---

#[test]
fn test_external_import_from() {
    let source = "tibble\n";
    let file = file_url("test.R");
    let idx = parse_source(source);
    let library = empty_library();

    let mut imports = HashMap::new();
    imports.insert("tibble".to_string(), "tibble".to_string());
    let scope_chain = vec![ScopeLayer::PackageImports(imports)];

    let targets = goto_definition(
        offset(0),
        &file,
        &idx,
        &ExternalScope::package(scope_chain.clone(), scope_chain),
        &library,
    );
    // importFrom resolves to a package, no file/range to navigate to
    assert!(targets.is_empty());
}

// --- Member access (`$`) ---

#[test]
fn test_dollar_lhs_resolves() {
    // Cursor on `foo` in `foo$bar` resolves to the definition of `foo`
    let source = "foo <- list()\nfoo$bar\n";
    let file = file_url("test.R");
    let idx = parse_source(source);
    let library = empty_library();

    // `foo` in `foo$bar` starts at offset 14
    let targets = goto_definition(offset(14), &file, &idx, &ExternalScope::default(), &library);
    assert_eq!(targets, vec![NavigationTarget {
        file,
        name: "foo".to_string(),
        full_range: text_range(0, 3),
        focus_range: text_range(0, 3),
    }]);
}

#[test]
fn test_dollar_rhs_no_resolution() {
    // Cursor on `bar` in `foo$bar`: member names are not tracked by the index
    let source = "foo <- list()\nfoo$bar\n";
    let file = file_url("test.R");
    let idx = parse_source(source);
    let library = empty_library();

    // `bar` starts at offset 18
    let targets = goto_definition(offset(18), &file, &idx, &ExternalScope::default(), &library);
    assert!(targets.is_empty());
}

// --- Use inside function body with cross-file definition ---

#[test]
fn test_use_in_function_body_resolves_via_external() {
    // Reproduces: `is_null` used inside a function body, defined in another
    // file. The use is free in the function scope, so resolution should fall
    // through enclosing scopes to the external scope chain.
    let source = "f <- function(x) {\n  if (is_null(x)) NULL\n}\n";
    let file = file_url("R/cnd-last.R");
    let idx = parse_source(source);

    let other_source = "is_null <- is.null\n";
    let other_idx = parse_source(other_source);
    let other_url = file_url("R/types.R");
    let scope_chain = file_layers(other_url.clone(), &other_idx);

    let library = empty_library();

    // `is_null` starts at offset 24
    let is_null_offset = source.find("is_null").unwrap();
    assert_eq!(is_null_offset, 25);

    let scope = ExternalScope::package(Vec::new(), scope_chain);

    let targets = goto_definition(offset(is_null_offset as u32), &file, &idx, &scope, &library);
    assert_eq!(targets, vec![NavigationTarget {
        file: other_url,
        name: "is_null".to_string(),
        full_range: text_range(0, 7),
        focus_range: text_range(0, 7),
    }]);
}

// --- No resolution ---

#[test]
fn test_no_use_at_offset() {
    let source = "x <- 1\n";
    let file = file_url("test.R");
    let idx = parse_source(source);
    let library = empty_library();

    let targets = goto_definition(offset(3), &file, &idx, &ExternalScope::default(), &library);
    assert!(targets.is_empty());
}

#[test]
fn test_unresolved_symbol() {
    let source = "foo\n";
    let file = file_url("test.R");
    let idx = parse_source(source);
    let library = empty_library();

    let targets = goto_definition(offset(0), &file, &idx, &ExternalScope::default(), &library);
    assert!(targets.is_empty());
}

// --- Local takes precedence over external ---

#[test]
fn test_local_shadows_external() {
    let source = "foo <- 1\nfoo\n";
    let file = file_url("test.R");
    let idx = parse_source(source);
    let library = test_library(vec![("pkg", vec!["foo"])]);

    let scope_chain = vec![ScopeLayer::PackageExports("pkg".to_string())];

    let use_offset = source.rfind("foo").unwrap() as u32;
    let targets = goto_definition(
        offset(use_offset),
        &file,
        &idx,
        &ExternalScope::package(scope_chain.clone(), scope_chain),
        &library,
    );
    assert_eq!(targets, vec![NavigationTarget {
        file,
        name: "foo".to_string(),
        full_range: text_range(0, 3),
        focus_range: text_range(0, 3),
    }]);
}

// --- Conditional definition (may_be_unbound but has local defs) ---

#[test]
fn test_conditional_definition_includes_external() {
    let source = "if (TRUE) x <- 1\nx\n";
    let file = file_url("test.R");
    let idx = parse_source(source);

    let other_url = file_url("other.R");
    let other_source = "x <- 99\n";
    let other_idx = parse_source(other_source);
    let scope_chain = file_layers(other_url.clone(), &other_idx);

    let library = empty_library();

    let use_offset = source.rfind('x').unwrap() as u32;
    let targets = goto_definition(
        offset(use_offset),
        &file,
        &idx,
        &ExternalScope::package(scope_chain.clone(), scope_chain),
        &library,
    );
    assert_eq!(targets, vec![
        NavigationTarget {
            file,
            name: "x".to_string(),
            full_range: text_range(10, 11),
            focus_range: text_range(10, 11),
        },
        NavigationTarget {
            file: other_url,
            name: "x".to_string(),
            full_range: text_range(0, 1),
            focus_range: text_range(0, 1),
        },
    ]);
}

// --- Definition site navigation ---

#[test]
fn test_definition_site_assignment() {
    // Cursor on the `foo` in `foo <- 1` should navigate to itself
    let source = "foo <- 1\n";
    let file = file_url("test.R");
    let idx = parse_source(source);
    let library = empty_library();

    let targets = goto_definition(offset(0), &file, &idx, &ExternalScope::default(), &library);
    assert_eq!(targets, vec![NavigationTarget {
        file,
        name: "foo".to_string(),
        full_range: text_range(0, 3),
        focus_range: text_range(0, 3),
    }]);
}

#[test]
fn test_definition_site_parameter() {
    let source = "f <- function(x) { x }\n";
    let file = file_url("test.R");
    let idx = parse_source(source);
    let library = empty_library();

    // Cursor on the `x` parameter name (offset 14)
    let targets = goto_definition(offset(14), &file, &idx, &ExternalScope::default(), &library);
    assert_eq!(targets, vec![NavigationTarget {
        file,
        name: "x".to_string(),
        full_range: text_range(14, 15),
        focus_range: text_range(14, 15),
    }]);
}

#[test]
fn test_definition_site_for_variable() {
    let source = "for (i in 1:10) i\n";
    let file = file_url("test.R");
    let idx = parse_source(source);
    let library = empty_library();

    // Cursor on the `i` in `for (i in ...)`
    let targets = goto_definition(offset(5), &file, &idx, &ExternalScope::default(), &library);
    assert_eq!(targets, vec![NavigationTarget {
        file,
        name: "i".to_string(),
        full_range: text_range(5, 6),
        focus_range: text_range(5, 6),
    }]);
}

// --- Right assignment ---

#[test]
fn test_right_assignment_definition_site() {
    // `1 -> x`: cursor on `x` (the definition target)
    let source = "1 -> x\nx\n";
    let file = file_url("test.R");
    let idx = parse_source(source);
    let library = empty_library();

    let targets = goto_definition(offset(5), &file, &idx, &ExternalScope::default(), &library);
    assert_eq!(targets, vec![NavigationTarget {
        file: file.clone(),
        name: "x".to_string(),
        full_range: text_range(5, 6),
        focus_range: text_range(5, 6),
    }]);
}

#[test]
fn test_right_assignment_use_resolves() {
    // `1 -> x` then use `x`
    let source = "1 -> x\nx\n";
    let file = file_url("test.R");
    let idx = parse_source(source);
    let library = empty_library();

    let targets = goto_definition(offset(7), &file, &idx, &ExternalScope::default(), &library);
    assert_eq!(targets, vec![NavigationTarget {
        file,
        name: "x".to_string(),
        full_range: text_range(5, 6),
        focus_range: text_range(5, 6),
    }]);
}

// --- Super assignment ---

#[test]
fn test_super_assignment_resolves_in_enclosing() {
    // `x <<- 1` inside a function creates a definition in the file scope.
    // A use of `x` in another function should resolve to it.
    let source = "f <- function() x <<- 1\ng <- function() x\n";
    let file = file_url("test.R");
    let idx = parse_source(source);
    let library = empty_library();

    // `x` use in `g` body
    let use_offset = source.rfind('x').unwrap() as u32;
    let targets = goto_definition(
        offset(use_offset),
        &file,
        &idx,
        &ExternalScope::default(),
        &library,
    );
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].name, "x");
    assert_eq!(targets[0].file, file);
}

#[test]
fn test_super_assignment_definition_site() {
    // Cursor on `x` in `x <<- 1`
    let source = "f <- function() {\n  x <<- 1\n}\n";
    let file = file_url("test.R");
    let idx = parse_source(source);
    let library = empty_library();

    // `x` at offset 20
    let def_offset = source.find("x <<-").unwrap() as u32;
    let targets = goto_definition(
        offset(def_offset),
        &file,
        &idx,
        &ExternalScope::default(),
        &library,
    );
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].name, "x");
}

// --- String definitions ---

#[test]
fn test_string_definition() {
    // `"foo" <- 1` is equivalent to `foo <- 1` in R
    let source = "\"foo\" <- 1\nfoo\n";
    let file = file_url("test.R");
    let idx = parse_source(source);
    let library = empty_library();

    // Use of `foo` at offset 11
    let targets = goto_definition(offset(11), &file, &idx, &ExternalScope::default(), &library);
    assert_eq!(targets, vec![NavigationTarget {
        file,
        name: "foo".to_string(),
        // The definition range covers the string literal `"foo"`
        full_range: text_range(0, 5),
        focus_range: text_range(0, 5),
    }]);
}

// --- Nested functions ---

#[test]
fn test_deeply_nested_function() {
    // Free variable `z` resolves through two function scopes to file scope
    let source = "z <- 1\nf <- function() {\n  g <- function() {\n    z\n  }\n}\n";
    let file = file_url("test.R");
    let idx = parse_source(source);
    let library = empty_library();

    let use_offset = source.rfind('z').unwrap() as u32;
    let targets = goto_definition(
        offset(use_offset),
        &file,
        &idx,
        &ExternalScope::default(),
        &library,
    );
    assert_eq!(targets, vec![NavigationTarget {
        file,
        name: "z".to_string(),
        full_range: text_range(0, 1),
        focus_range: text_range(0, 1),
    }]);
}

// --- Use on RHS of assignment ---

#[test]
fn test_use_on_rhs_of_assignment() {
    // `x <- x + 1`: the `x` on the RHS refers to the previous binding
    let source = "x <- 1\nx <- x + 1\n";
    let file = file_url("test.R");
    let idx = parse_source(source);
    let library = empty_library();

    // The `x` on the RHS of the second assignment. `x <- x + 1` starts at
    // offset 7, the RHS `x` is at offset 12.
    let rhs_offset = 7 + "x <- ".len() as u32;
    let targets = goto_definition(
        offset(rhs_offset),
        &file,
        &idx,
        &ExternalScope::default(),
        &library,
    );
    assert_eq!(targets, vec![NavigationTarget {
        file,
        name: "x".to_string(),
        // Resolves to the first definition
        full_range: text_range(0, 1),
        focus_range: text_range(0, 1),
    }]);
}

// --- library() directive in predecessor file ---

#[test]
fn test_library_directive_in_predecessor() {
    // aaa.R has `library(dplyr)`, bbb.R uses `mutate`.
    // The library() directive in aaa.R should make dplyr exports visible.
    let aaa_source = "library(dplyr)\n";
    let aaa_idx = parse_source(aaa_source);
    let aaa_url = file_url("R/aaa.R");

    let bbb_source = "mutate\n";
    let bbb_url = file_url("R/bbb.R");
    let bbb_idx = parse_source(bbb_source);

    let aaa_layers = file_layers(aaa_url, &aaa_idx);
    let library = test_library(vec![("dplyr", vec!["filter", "mutate", "select"])]);

    let scope = ExternalScope::package(aaa_layers.clone(), aaa_layers);

    let targets = goto_definition(offset(0), &bbb_url, &bbb_idx, &scope, &library);
    // dplyr::mutate is a package symbol, no file/range to navigate to
    assert!(targets.is_empty());
}
