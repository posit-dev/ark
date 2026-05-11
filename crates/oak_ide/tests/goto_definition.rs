use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use aether_parser::parse;
use aether_parser::RParserOptions;
use aether_syntax::RSyntaxNode;
use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_db::LegacyDb;
use oak_ide::goto_definition;
use oak_ide::ExternalScope;
use oak_ide::NavigationTarget;
use oak_package_metadata::description::Description;
use oak_package_metadata::namespace::Namespace;
use oak_semantic::library::Library;
use oak_semantic::package::Package;
use oak_semantic::scope_layer::file_layers;
use oak_semantic::scope_layer::ScopeLayer;
use oak_semantic::semantic_index;
use oak_semantic::semantic_index::SemanticCallKind;
use oak_semantic::semantic_index::SemanticIndex;
use oak_semantic::semantic_index_with_source_resolver;
use oak_semantic::ScopeId;
use oak_semantic::SourceResolution;
use oak_sources::test::TestPackageCache;
use stdext::SortedVec;
use url::Url;

fn parse_source(source: &str) -> (RSyntaxNode, SemanticIndex) {
    let parsed = parse(source, RParserOptions::default());
    let root = parsed.syntax();
    let index = semantic_index(&parsed.tree(), &file_url("test.R"));
    (root, index)
}

struct TestDb {
    library: Library,
    sources: HashMap<Url, String>,
}

impl LegacyDb for TestDb {
    fn semantic_index(&self, file: &Url) -> Option<SemanticIndex> {
        // Rebuild from source for tests. We store the source instead.
        self.sources.get(file).map(|source| {
            let parsed = parse(source, RParserOptions::default());
            semantic_index(&parsed.tree(), file)
        })
    }
    fn library(&self) -> &Library {
        &self.library
    }
}

fn empty_library() -> Library {
    Library::new(vec![], None)
}

struct TestPackage {
    name: String,
    file: PathBuf,
    exports: Vec<String>,
    internals: Vec<String>,
}

impl TestPackage {
    // Most convenient input types for test construction
    fn new(name: &str, file: &str, exports: Vec<&str>, internals: Vec<&str>) -> Self {
        Self {
            name: String::from(name),
            file: PathBuf::from(file),
            exports: exports
                .into_iter()
                .map(|export| export.to_string())
                .collect(),
            internals: internals
                .into_iter()
                .map(|export| export.to_string())
                .collect(),
        }
    }
}

fn test_library(packages: Vec<TestPackage>) -> Library {
    let cache = TestPackageCache::new().unwrap();

    // Both exports and internals are in the test file
    for package in &packages {
        let content = test_file(&package.exports, &package.internals);
        cache
            .add(&package.name, vec![(package.file.as_path(), &content)])
            .expect("Can write to cache");
    }

    let cache = Arc::new(cache);

    let mut library = Library::new(vec![], Some(cache));

    // Only exports are included in `Namespace`
    for package in &packages {
        let ns = Namespace {
            exports: SortedVec::from_vec(package.exports.clone()),
            ..Default::default()
        };
        let desc = Description {
            name: package.name.clone(),
            ..Default::default()
        };
        let pkg = Package::from_parts(PathBuf::from("/fake"), desc, ns);
        library = library.insert(&package.name, pkg);
    }

    library
}

// Create a file worth of function definitions
fn test_file(exports: &Vec<String>, internals: &Vec<String>) -> String {
    let mut out = String::new();

    for export in exports {
        out.push_str(&format!("{export} <- function() {{}}\n\n"));
    }
    for internal in internals {
        out.push_str(&format!("{internal} <- function() {{}}\n\n"));
    }

    out
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
    let (root, idx) = parse_source(source);
    let db = TestDb {
        library: empty_library(),
        sources: HashMap::new(),
    };

    let targets = goto_definition(
        &db,
        offset(7),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
    );
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
    let (root, idx) = parse_source(source);
    let db = TestDb {
        library: empty_library(),
        sources: HashMap::new(),
    };

    let targets = goto_definition(
        &db,
        offset(14),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
    );
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
    let (root, idx) = parse_source(source);
    let db = TestDb {
        library: empty_library(),
        sources: HashMap::new(),
    };

    let use_offset = source.rfind('x').unwrap() as u32;

    let targets = goto_definition(
        &db,
        offset(use_offset),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
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
    let (root, idx) = parse_source(source);
    let db = TestDb {
        library: empty_library(),
        sources: HashMap::new(),
    };

    let use_offset = source.rfind('x').unwrap() as u32;
    let targets = goto_definition(
        &db,
        offset(use_offset),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
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
    let (root, idx) = parse_source(source);
    let db = TestDb {
        library: empty_library(),
        sources: HashMap::new(),
    };

    let use_offset = source.rfind('x').unwrap() as u32;
    let targets = goto_definition(
        &db,
        offset(use_offset),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
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
    let (root, idx) = parse_source(source);
    let db = TestDb {
        library: empty_library(),
        sources: HashMap::new(),
    };

    let use_offset = source.rfind('x').unwrap() as u32;
    let targets = goto_definition(
        &db,
        offset(use_offset),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
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
    let (root, idx) = parse_source(source);
    let db = TestDb {
        library: empty_library(),
        sources: HashMap::new(),
    };

    let other_url = file_url("other.R");
    let other_source = "foo <- 42\n";
    let (_other_root, other_idx) = parse_source(other_source);
    let scope_chain = file_layers(other_url.clone(), &other_idx);

    let targets = goto_definition(
        &db,
        offset(0),
        &file,
        &root,
        &idx,
        &ExternalScope::package(scope_chain.clone(), scope_chain),
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
    let (root, idx) = parse_source(source);
    let library = test_library(vec![TestPackage::new(
        "dplyr",
        "dplyr.R",
        vec!["filter", "mutate", "select"],
        vec![],
    )]);
    let db = TestDb {
        library,
        sources: HashMap::new(),
    };

    let scope_chain = vec![ScopeLayer::PackageExports("dplyr".to_string())];

    let targets = goto_definition(
        &db,
        offset(0),
        &file,
        &root,
        &idx,
        &ExternalScope::package(scope_chain.clone(), scope_chain),
    );

    assert_eq!(targets.len(), 1);

    let target = targets.first().unwrap();
    assert!(target.file.path().ends_with("dplyr.R"));
    assert_eq!(target.name, "mutate".to_string());
}

// --- External resolution: importFrom ---

#[test]
fn test_external_import_from() {
    let source = "tibble\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);
    let db = TestDb {
        library: empty_library(),
        sources: HashMap::new(),
    };

    let mut imports = HashMap::new();
    imports.insert("tibble".to_string(), "tibble".to_string());
    let scope_chain = vec![ScopeLayer::PackageImports(imports)];

    let targets = goto_definition(
        &db,
        offset(0),
        &file,
        &root,
        &idx,
        &ExternalScope::package(scope_chain.clone(), scope_chain),
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
    let (root, idx) = parse_source(source);
    let db = TestDb {
        library: empty_library(),
        sources: HashMap::new(),
    };

    // `foo` in `foo$bar` starts at offset 14
    let targets = goto_definition(
        &db,
        offset(14),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
    );
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
    let (root, idx) = parse_source(source);
    let db = TestDb {
        library: empty_library(),
        sources: HashMap::new(),
    };

    // `bar` starts at offset 18
    let targets = goto_definition(
        &db,
        offset(18),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
    );
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
    let (root, idx) = parse_source(source);

    let other_source = "is_null <- is.null\n";
    let (_other_root, other_idx) = parse_source(other_source);
    let other_url = file_url("R/types.R");
    let scope_chain = file_layers(other_url.clone(), &other_idx);

    let db = TestDb {
        library: empty_library(),
        sources: HashMap::new(),
    };

    // `is_null` starts at offset 24
    let is_null_offset = source.find("is_null").unwrap();
    assert_eq!(is_null_offset, 25);

    let scope = ExternalScope::package(Vec::new(), scope_chain);

    let targets = goto_definition(
        &db,
        offset(is_null_offset as u32),
        &file,
        &root,
        &idx,
        &scope,
    );
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
    let (root, idx) = parse_source(source);
    let db = TestDb {
        library: empty_library(),
        sources: HashMap::new(),
    };

    let targets = goto_definition(
        &db,
        offset(3),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
    );
    assert!(targets.is_empty());
}

#[test]
fn test_unresolved_symbol() {
    let source = "foo\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);
    let db = TestDb {
        library: empty_library(),
        sources: HashMap::new(),
    };

    let targets = goto_definition(
        &db,
        offset(0),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
    );
    assert!(targets.is_empty());
}

// --- Local takes precedence over external ---

#[test]
fn test_local_shadows_external() {
    let source = "foo <- 1\nfoo\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);
    let library = test_library(vec![TestPackage::new("pkg", "pkg.R", vec!["foo"], vec![])]);
    let db = TestDb {
        library,
        sources: HashMap::new(),
    };

    let scope_chain = vec![ScopeLayer::PackageExports("pkg".to_string())];

    let use_offset = source.rfind("foo").unwrap() as u32;
    let targets = goto_definition(
        &db,
        offset(use_offset),
        &file,
        &root,
        &idx,
        &ExternalScope::package(scope_chain.clone(), scope_chain),
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
    let (root, idx) = parse_source(source);

    let other_url = file_url("other.R");
    let other_source = "x <- 99\n";
    let (_other_root, other_idx) = parse_source(other_source);
    let scope_chain = file_layers(other_url.clone(), &other_idx);

    let db = TestDb {
        library: empty_library(),
        sources: HashMap::new(),
    };

    let use_offset = source.rfind('x').unwrap() as u32;
    let targets = goto_definition(
        &db,
        offset(use_offset),
        &file,
        &root,
        &idx,
        &ExternalScope::package(scope_chain.clone(), scope_chain),
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
    let (root, idx) = parse_source(source);
    let db = TestDb {
        library: empty_library(),
        sources: HashMap::new(),
    };

    let targets = goto_definition(
        &db,
        offset(0),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
    );
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
    let (root, idx) = parse_source(source);
    let db = TestDb {
        library: empty_library(),
        sources: HashMap::new(),
    };

    // Cursor on the `x` parameter name (offset 14)
    let targets = goto_definition(
        &db,
        offset(14),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
    );
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
    let (root, idx) = parse_source(source);
    let db = TestDb {
        library: empty_library(),
        sources: HashMap::new(),
    };

    // Cursor on the `i` in `for (i in ...)`
    let targets = goto_definition(
        &db,
        offset(5),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
    );
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
    let (root, idx) = parse_source(source);
    let db = TestDb {
        library: empty_library(),
        sources: HashMap::new(),
    };

    let targets = goto_definition(
        &db,
        offset(5),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
    );
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
    let (root, idx) = parse_source(source);
    let db = TestDb {
        library: empty_library(),
        sources: HashMap::new(),
    };

    let targets = goto_definition(
        &db,
        offset(7),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
    );
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
    let (root, idx) = parse_source(source);
    let db = TestDb {
        library: empty_library(),
        sources: HashMap::new(),
    };

    // `x` use in `g` body
    let use_offset = source.rfind('x').unwrap() as u32;
    let targets = goto_definition(
        &db,
        offset(use_offset),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
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
    let (root, idx) = parse_source(source);
    let db = TestDb {
        library: empty_library(),
        sources: HashMap::new(),
    };

    // `x` at offset 20
    let def_offset = source.find("x <<-").unwrap() as u32;
    let targets = goto_definition(
        &db,
        offset(def_offset),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
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
    let (root, idx) = parse_source(source);
    let db = TestDb {
        library: empty_library(),
        sources: HashMap::new(),
    };

    // Use of `foo` at offset 11
    let targets = goto_definition(
        &db,
        offset(11),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
    );
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
    let (root, idx) = parse_source(source);
    let db = TestDb {
        library: empty_library(),
        sources: HashMap::new(),
    };

    let use_offset = source.rfind('z').unwrap() as u32;
    let targets = goto_definition(
        &db,
        offset(use_offset),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
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
    let (root, idx) = parse_source(source);
    let db = TestDb {
        library: empty_library(),
        sources: HashMap::new(),
    };

    // The `x` on the RHS of the second assignment. `x <- x + 1` starts at
    // offset 7, the RHS `x` is at offset 12.
    let rhs_offset = 7 + "x <- ".len() as u32;
    let targets = goto_definition(
        &db,
        offset(rhs_offset),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
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
    let (_aaa_root, aaa_idx) = parse_source(aaa_source);
    let aaa_url = file_url("R/aaa.R");

    let bbb_source = "mutate\n";
    let bbb_url = file_url("R/bbb.R");
    let (bbb_root, bbb_idx) = parse_source(bbb_source);

    let aaa_layers = file_layers(aaa_url, &aaa_idx);
    let library = test_library(vec![TestPackage::new(
        "dplyr",
        "dplyr.R",
        vec!["filter", "mutate", "select"],
        vec![],
    )]);
    let db = TestDb {
        library,
        sources: HashMap::new(),
    };

    let scope = ExternalScope::package(aaa_layers.clone(), aaa_layers);

    let targets = goto_definition(&db, offset(0), &bbb_url, &bbb_root, &bbb_idx, &scope);

    assert_eq!(targets.len(), 1);

    let target = targets.first().unwrap();
    assert!(target.file.path().ends_with("dplyr.R"));
    assert_eq!(target.name, "mutate".to_string());
}

// --- Namespace access (:: and :::) ---

#[test]
fn test_namespace_access_exported_symbol() {
    // "dplyr::mutate\n"
    //  0123456789...
    // Cursor on `mutate` (offset 7)
    let source = "dplyr::mutate\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);
    let library = test_library(vec![TestPackage::new(
        "dplyr",
        "dplyr.R",
        vec!["filter", "mutate", "select"],
        vec![],
    )]);
    let db = TestDb {
        library,
        sources: HashMap::new(),
    };

    let targets = goto_definition(
        &db,
        offset(7),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
    );

    assert_eq!(targets.len(), 1);

    let target = targets.first().unwrap();
    assert!(target.file.path().ends_with("dplyr.R"));
    assert_eq!(target.name, "mutate".to_string());
}

#[test]
fn test_namespace_access_unknown_symbol() {
    // Symbol not in exports
    let source = "dplyr::nonexistent\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);
    let library = test_library(vec![TestPackage::new(
        "dplyr",
        "dplyr.R",
        vec!["filter", "mutate", "select"],
        vec![],
    )]);
    let db = TestDb {
        library,
        sources: HashMap::new(),
    };

    let targets = goto_definition(
        &db,
        offset(7),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
    );
    assert!(targets.is_empty());
}

#[test]
fn test_namespace_access_unknown_package() {
    let source = "bogus::foo\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);
    let library = test_library(vec![]);
    let db = TestDb {
        library,
        sources: HashMap::new(),
    };

    let targets = goto_definition(
        &db,
        offset(7),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
    );
    assert!(targets.is_empty());
}

#[test]
fn test_namespace_access_triple_colon() {
    let library = test_library(vec![TestPackage::new(
        "pkg",
        "pkg.R",
        vec!["external_fn"],
        vec!["internal_fn"],
    )]);
    let db = TestDb {
        library,
        sources: HashMap::new(),
    };

    // "pkg:::internal_fn\n"
    //  01234567890...
    // Cursor on `internal_fn` (offset 6)
    let source = "pkg:::internal_fn\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);
    let targets = goto_definition(
        &db,
        offset(6),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
    );
    assert_eq!(targets.len(), 1);
    let target = targets.first().unwrap();
    assert!(target.file.path().ends_with("pkg.R"));
    assert_eq!(target.name, "internal_fn".to_string());

    // "pkg:::external_fn\n"
    //  01234567890...
    // Cursor on `external_fn` (offset 6)
    let source = "pkg:::external_fn\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);
    let targets = goto_definition(
        &db,
        offset(6),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
    );
    assert_eq!(targets.len(), 1);
    let target = targets.first().unwrap();
    assert!(target.file.path().ends_with("pkg.R"));
    assert_eq!(target.name, "external_fn".to_string());
}

#[test]
fn test_fixme_namespace_access_cursor_on_package_name() {
    // Cursor on `dplyr` (offset 0) — the LHS of ::
    let source = "dplyr::mutate\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);
    let library = test_library(vec![TestPackage::new(
        "dplyr",
        "dplyr.R",
        vec!["filter", "mutate", "select"],
        vec![],
    )]);
    let db = TestDb {
        library,
        sources: HashMap::new(),
    };

    let targets = goto_definition(
        &db,
        offset(0),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
    );
    // FIXME: Cursor on the package name still classifies as NamespaceAccess
    // and resolves `mutate` in `dplyr`.
    assert_eq!(targets.len(), 1);
    let target = targets.first().unwrap();
    assert!(target.file.path().ends_with("dplyr.R"));
    assert_eq!(target.name, "mutate".to_string());
}

#[test]
fn test_fixme_namespace_access_cursor_on_operator() {
    // Cursor on `::` (offset 5)
    let source = "dplyr::mutate\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);
    let library = test_library(vec![TestPackage::new(
        "dplyr",
        "dplyr.R",
        vec!["filter", "mutate", "select"],
        vec![],
    )]);
    let db = TestDb {
        library,
        sources: HashMap::new(),
    };

    let targets = goto_definition(
        &db,
        offset(5),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
    );
    // FIXME: Operator token is inside the RNamespaceExpression, still resolves.
    assert_eq!(targets.len(), 1);
    let target = targets.first().unwrap();
    assert!(target.file.path().ends_with("dplyr.R"));
    assert_eq!(target.name, "mutate".to_string());
}

#[test]
fn test_namespace_classify() {
    use oak_ide::Identifier;

    let source = "dplyr::mutate\n";
    let parsed = parse(source, RParserOptions::default());
    let root = parsed.syntax();
    let idx = semantic_index(&parsed.tree(), &file_url("test.R"));

    // Cursor on `mutate` (offset 7)
    let ident = Identifier::classify(&root, &idx, offset(7));
    assert_eq!(
        ident,
        Some(Identifier::NamespaceAccess {
            package: "dplyr".to_string(),
            symbol: "mutate".to_string(),
            internal: false,
            package_range: text_range(0, 5),
            symbol_range: text_range(7, 13),
        })
    );

    // Cursor on `dplyr` (offset 2)
    let ident = Identifier::classify(&root, &idx, offset(2));
    assert_eq!(
        ident,
        Some(Identifier::NamespaceAccess {
            package: "dplyr".to_string(),
            symbol: "mutate".to_string(),
            internal: false,
            package_range: text_range(0, 5),
            symbol_range: text_range(7, 13),
        })
    );
}

#[test]
fn test_namespace_classify_triple_colon() {
    use oak_ide::Identifier;

    let source = "pkg:::sym\n";
    let parsed = parse(source, RParserOptions::default());
    let root = parsed.syntax();
    let idx = semantic_index(&parsed.tree(), &file_url("test.R"));

    let ident = Identifier::classify(&root, &idx, offset(6));
    assert_eq!(
        ident,
        Some(Identifier::NamespaceAccess {
            package: "pkg".to_string(),
            symbol: "sym".to_string(),
            internal: true,
            package_range: text_range(0, 3),
            symbol_range: text_range(6, 9),
        })
    );
}

#[test]
fn test_namespace_access_in_call() {
    // foo::bar() — cursor on `bar`
    let source = "foo::bar()\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);
    let library = test_library(vec![TestPackage::new("foo", "foo.R", vec!["bar"], vec![])]);
    let db = TestDb {
        library,
        sources: HashMap::new(),
    };

    let targets = goto_definition(
        &db,
        offset(5),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
    );

    assert_eq!(targets.len(), 1);

    let target = targets.first().unwrap();
    assert!(target.file.path().ends_with("foo.R"));
    assert_eq!(target.name, "bar".to_string());
}

#[test]
fn test_namespace_access_in_extract() {
    // foo::bar$baz — cursor on `bar`
    let source = "foo::bar$baz\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);
    let library = test_library(vec![TestPackage::new("foo", "foo.R", vec!["bar"], vec![])]);
    let db = TestDb {
        library,
        sources: HashMap::new(),
    };

    let targets = goto_definition(
        &db,
        offset(5),
        &file,
        &root,
        &idx,
        &ExternalScope::default(),
    );

    assert_eq!(targets.len(), 1);

    let target = targets.first().unwrap();
    assert!(target.file.path().ends_with("foo.R"));
    assert_eq!(target.name, "bar".to_string());
}

#[test]
fn test_namespace_classify_in_call() {
    use oak_ide::Identifier;

    // foo::bar()
    // 0123456789
    let source = "foo::bar()\n";
    let parsed = parse(source, RParserOptions::default());
    let root = parsed.syntax();
    let idx = semantic_index(&parsed.tree(), &file_url("test.R"));

    let ident = Identifier::classify(&root, &idx, offset(5));
    assert_eq!(
        ident,
        Some(Identifier::NamespaceAccess {
            package: "foo".to_string(),
            symbol: "bar".to_string(),
            internal: false,
            package_range: text_range(0, 3),
            symbol_range: text_range(5, 8),
        })
    );
}

#[test]
fn test_namespace_classify_in_extract() {
    use oak_ide::Identifier;

    // foo::bar$baz
    // 0123456789...
    let source = "foo::bar$baz\n";
    let parsed = parse(source, RParserOptions::default());
    let root = parsed.syntax();
    let idx = semantic_index(&parsed.tree(), &file_url("test.R"));

    // Cursor on `bar` (offset 5) — inside the RNamespaceExpression
    let ident = Identifier::classify(&root, &idx, offset(5));
    assert_eq!(
        ident,
        Some(Identifier::NamespaceAccess {
            package: "foo".to_string(),
            symbol: "bar".to_string(),
            internal: false,
            package_range: text_range(0, 3),
            symbol_range: text_range(5, 8),
        })
    );

    // Cursor on `baz` (offset 9) — RHS of $, not a namespace access
    let ident = Identifier::classify(&root, &idx, offset(9));
    assert_eq!(ident, None);
}

#[test]
fn test_namespace_classify_string_selectors() {
    use oak_ide::Identifier;

    // "foo"::"bar"
    //  0123456789...
    let source = "\"foo\"::\"bar\"\n";
    let parsed = parse(source, RParserOptions::default());
    let root = parsed.syntax();
    let idx = semantic_index(&parsed.tree(), &file_url("test.R"));

    let ident = Identifier::classify(&root, &idx, offset(7));
    assert_eq!(
        ident,
        Some(Identifier::NamespaceAccess {
            package: "foo".to_string(),
            symbol: "bar".to_string(),
            internal: false,
            package_range: text_range(0, 5),
            symbol_range: text_range(7, 12),
        })
    );
}

// --- source() directive ---

#[test]
fn test_source_directive_resolves_to_sourced_file() {
    // script.R has `source("helpers.R")` then uses `helper`.
    // The builder resolves source() via the callback and injects the
    // sourced file's exports, enabling goto-definition.
    let helpers_source = "helper <- function() 1\n";
    let helpers_url = file_url("helpers.R");
    let (_helpers_root, helpers_idx) = parse_source(helpers_source);

    let script_source = "source(\"helpers.R\")\nhelper\n";
    let script_url = file_url("script.R");

    let helpers_names: Vec<String> = helpers_idx
        .file_exports()
        .keys()
        .map(|name| name.to_string())
        .collect();

    let helpers_url_clone = helpers_url.clone();
    let parsed = parse(script_source, RParserOptions::default());
    let script_root = parsed.syntax();
    let script_idx =
        semantic_index_with_source_resolver(&parsed.tree(), &script_url, move |_path| {
            Some(SourceResolution {
                file: helpers_url_clone.clone(),
                names: helpers_names.clone(),
                packages: Vec::new(),
            })
        });

    let dir_layers = script_idx.semantic_calls().to_vec();
    let scope = ExternalScope::search_path(dir_layers, Vec::new());

    let mut sources = HashMap::new();
    sources.insert(helpers_url.clone(), helpers_source.to_string());
    let db = TestDb {
        library: empty_library(),
        sources,
    };

    let use_offset = script_source.rfind("helper").unwrap() as u32;
    let targets = goto_definition(
        &db,
        offset(use_offset),
        &script_url,
        &script_root,
        &script_idx,
        &scope,
    );
    assert_eq!(targets, vec![NavigationTarget {
        file: helpers_url,
        name: "helper".to_string(),
        full_range: text_range(0, 6),
        focus_range: text_range(0, 6),
    }]);
}

#[test]
fn test_source_directive_resolves_nested_library() {
    // helpers.R has `library(dplyr)` and defines `helper`.
    // script.R sources helpers.R then uses `mutate` (from dplyr).
    // The nested library() directive should be visible via the resolver.
    let helpers_source = "library(dplyr)\nhelper <- function() 1\n";
    let helpers_url = file_url("helpers.R");
    let (_helpers_root, helpers_idx) = parse_source(helpers_source);

    let helpers_names: Vec<String> = helpers_idx
        .file_exports()
        .keys()
        .map(|name| name.to_string())
        .collect();
    let helpers_packages: Vec<_> = helpers_idx
        .semantic_calls()
        .iter()
        .filter_map(|c| match c.kind() {
            SemanticCallKind::Attach { package } => Some(package.clone()),
            SemanticCallKind::Source { .. } => None,
        })
        .collect();

    let script_source = "source(\"helpers.R\")\nmutate\n";
    let script_url = file_url("script.R");

    let library = test_library(vec![TestPackage::new(
        "dplyr",
        "dplyr.R",
        vec!["filter", "mutate", "select"],
        vec![],
    )]);

    let helpers_url_clone = helpers_url.clone();
    let names_clone = helpers_names.clone();
    let packages_clone = helpers_packages.clone();
    let parsed = parse(script_source, RParserOptions::default());
    let script_root = parsed.syntax();
    let script_idx =
        semantic_index_with_source_resolver(&parsed.tree(), &script_url, move |_path| {
            Some(SourceResolution {
                file: helpers_url_clone.clone(),
                names: names_clone.clone(),
                packages: packages_clone.clone(),
            })
        });

    let dir_layers = script_idx.semantic_calls().to_vec();
    let scope = ExternalScope::search_path(dir_layers, Vec::new());

    let mut sources = HashMap::new();
    sources.insert(helpers_url.clone(), helpers_source.to_string());
    let db = TestDb { library, sources };

    // `mutate` resolves via dplyr (attached by helpers.R's library() call)
    let use_offset = script_source.rfind("mutate").unwrap() as u32;
    let targets = goto_definition(
        &db,
        offset(use_offset),
        &script_url,
        &script_root,
        &script_idx,
        &scope,
    );
    // `mutate` resolves via dplyr (attached by helpers.R's library() call)
    assert!(!targets.is_empty());
    let source_with_helper = "source(\"helpers.R\")\nhelper\n";

    let helpers_url_clone = helpers_url.clone();
    let names_clone = helpers_names.clone();
    let packages_clone = helpers_packages.clone();
    let parsed2 = parse(source_with_helper, RParserOptions::default());
    let script_root2 = parsed2.syntax();
    let script_idx2 =
        semantic_index_with_source_resolver(&parsed2.tree(), &script_url, move |_path| {
            Some(SourceResolution {
                file: helpers_url_clone.clone(),
                names: names_clone.clone(),
                packages: packages_clone.clone(),
            })
        });

    let dir_layers = script_idx2.semantic_calls().to_vec();
    let scope = ExternalScope::search_path(dir_layers, Vec::new());

    let use_offset = source_with_helper.rfind("helper").unwrap() as u32;
    let targets = goto_definition(
        &db,
        offset(use_offset),
        &script_url,
        &script_root2,
        &script_idx2,
        &scope,
    );
    assert_eq!(targets, vec![NavigationTarget {
        file: helpers_url,
        name: "helper".to_string(),
        full_range: text_range(15, 21),
        focus_range: text_range(15, 21),
    }]);
}

#[test]
fn test_directive_not_visible_before_call_site() {
    // Directives are position-stamped: only code AFTER a `source()` or
    // `library()` call sees its effects.
    //
    //  "mutate\n"                     offset 0..6
    //  "helper\n"                     offset 7..13
    //  "library(dplyr)\n"             offset 14..28
    //  "source(\"helpers.R\")\n"      offset 29..48
    //  "mutate\n"                     offset 49..55
    //  "helper\n"                     offset 56..62
    let helpers_source = "helper <- function() 1\n";
    let helpers_url = file_url("helpers.R");
    let (_helpers_root, helpers_idx) = parse_source(helpers_source);

    let script_source = "mutate\nhelper\nlibrary(dplyr)\nsource(\"helpers.R\")\nmutate\nhelper\n";
    let script_url = file_url("script.R");

    let library = test_library(vec![TestPackage::new(
        "dplyr",
        "dplyr.R",
        vec!["filter", "mutate", "select"],
        vec![],
    )]);

    let helpers_url_clone = helpers_url.clone();
    let helpers_names: Vec<String> = helpers_idx
        .file_exports()
        .keys()
        .map(|name| name.to_string())
        .collect();

    let parsed = parse(script_source, RParserOptions::default());
    let script_root = parsed.syntax();
    let script_idx =
        semantic_index_with_source_resolver(&parsed.tree(), &script_url, move |_path| {
            Some(SourceResolution {
                file: helpers_url_clone.clone(),
                names: helpers_names.clone(),
                packages: Vec::new(),
            })
        });

    let dir_layers = script_idx.semantic_calls().to_vec();
    let scope = ExternalScope::search_path(dir_layers, Vec::new());

    let mut sources = HashMap::new();
    sources.insert(helpers_url.clone(), helpers_source.to_string());
    let db = TestDb { library, sources };

    // `mutate` before library(dplyr) (offset 0) — should NOT resolve
    let targets = goto_definition(
        &db,
        offset(0),
        &script_url,
        &script_root,
        &script_idx,
        &scope,
    );
    assert!(targets.is_empty());

    // `helper` before source() (offset 7) — should NOT resolve
    let targets = goto_definition(
        &db,
        offset(7),
        &script_url,
        &script_root,
        &script_idx,
        &scope,
    );
    assert!(targets.is_empty());

    // `mutate` after library(dplyr) (offset 49) — resolves via dplyr
    let targets = goto_definition(
        &db,
        offset(49),
        &script_url,
        &script_root,
        &script_idx,
        &scope,
    );
    assert!(!targets.is_empty());

    // `helper` after source() (offset 56) — should resolve to helpers.R
    let targets = goto_definition(
        &db,
        offset(56),
        &script_url,
        &script_root,
        &script_idx,
        &scope,
    );
    assert_eq!(targets, vec![NavigationTarget {
        file: helpers_url,
        name: "helper".to_string(),
        full_range: text_range(0, 6),
        focus_range: text_range(0, 6),
    }]);
}

#[test]
fn test_directives_in_function_body_are_scoped() {
    // `library()` inside a function body produces a scoped Attach
    // semantic call: visible inside the function but not at file scope.
    // The `source()` call is recorded as a Source semantic call,
    // independent of the legacy resolver path.
    let script_source =
        "f <- function() {\n  source(\"helpers.R\")\n  library(dplyr)\n  mutate\n}\nhelper\nmutate\n";
    let script_url = file_url("script.R");
    let (script_root, script_idx) = parse_source(script_source);

    let library = test_library(vec![TestPackage::new(
        "dplyr",
        "dplyr.R",
        vec!["filter", "mutate", "select"],
        vec![],
    )]);
    let db = TestDb {
        library,
        sources: HashMap::new(),
    };

    // Both source() and library() inside f are recorded as scoped
    // semantic calls, in source order.
    let semantic_calls = script_idx.semantic_calls();
    assert_eq!(semantic_calls.len(), 2);
    assert_eq!(semantic_calls[0].kind(), &SemanticCallKind::Source {
        path: "helpers.R".into()
    });
    assert_eq!(semantic_calls[1].kind(), &SemanticCallKind::Attach {
        package: "dplyr".into()
    });
    assert_ne!(semantic_calls[0].scope(), ScopeId::from(0));
    assert_ne!(semantic_calls[1].scope(), ScopeId::from(0));

    let dir_layers = script_idx.semantic_calls().to_vec();
    let scope = ExternalScope::search_path(dir_layers, Vec::new());

    // `mutate` inside f (after library()) — resolves via scoped dplyr
    let use_offset = script_source.find("  mutate").unwrap() as u32 + 2;
    let targets = goto_definition(
        &db,
        offset(use_offset),
        &script_url,
        &script_root,
        &script_idx,
        &scope,
    );
    // `mutate` inside f (after library()) — resolves via scoped dplyr
    assert!(!targets.is_empty());

    // `helper` at file scope — not resolved (source() had no resolver)
    let use_offset = script_source.find("\nhelper").unwrap() as u32 + 1;
    let targets = goto_definition(
        &db,
        offset(use_offset),
        &script_url,
        &script_root,
        &script_idx,
        &scope,
    );
    assert!(targets.is_empty());

    // `mutate` at file scope — not resolved (library() directive is
    // scoped to f, not visible here)
    let use_offset = script_source.rfind("mutate").unwrap() as u32;
    let targets = goto_definition(
        &db,
        offset(use_offset),
        &script_url,
        &script_root,
        &script_idx,
        &scope,
    );
    assert!(targets.is_empty());
}

#[test]
fn test_source_in_function_body_scoping() {
    // `source(local = FALSE)` inside a function body scopes directives to the
    // function scope, so sourced definitions are NOT visible at file scope.
    let helpers_source = "helper <- function() 1\n";
    let helpers_url = file_url("helpers.R");
    let (_helpers_root, helpers_idx) = parse_source(helpers_source);

    let script_source = "f <- function() {\n  source(\"helpers.R\")\n  helper\n}\nhelper\n";
    let script_url = file_url("script.R");

    let helpers_url_clone = helpers_url.clone();
    let helpers_names: Vec<String> = helpers_idx
        .file_exports()
        .keys()
        .map(|name| name.to_string())
        .collect();

    let parsed = parse(script_source, RParserOptions::default());
    let script_root = parsed.syntax();
    let script_idx =
        semantic_index_with_source_resolver(&parsed.tree(), &script_url, move |_path| {
            Some(SourceResolution {
                file: helpers_url_clone.clone(),
                names: helpers_names.clone(),
                packages: Vec::new(),
            })
        });

    let dir_layers = script_idx.semantic_calls().to_vec();
    let scope = ExternalScope::search_path(dir_layers, Vec::new());

    let mut sources = HashMap::new();
    sources.insert(helpers_url.clone(), helpers_source.to_string());
    let db = TestDb {
        library: empty_library(),
        sources,
    };

    // `helper` inside the function body — should resolve to helpers.R
    let inner_offset = script_source.find("  helper\n}").unwrap() as u32 + 2;
    let targets = goto_definition(
        &db,
        offset(inner_offset),
        &script_url,
        &script_root,
        &script_idx,
        &scope,
    );
    assert_eq!(targets, vec![NavigationTarget {
        file: helpers_url,
        name: "helper".to_string(),
        full_range: text_range(0, 6),
        focus_range: text_range(0, 6),
    }]);

    // `helper` outside the function — NOT visible
    let outer_offset = script_source.rfind("\nhelper\n").unwrap() as u32 + 1;
    let targets = goto_definition(
        &db,
        offset(outer_offset),
        &script_url,
        &script_root,
        &script_idx,
        &scope,
    );
    assert!(targets.is_empty());
}

// TODO(salsa): This tests `resolve_import` which is slated to move to
// `oak_semantic`. Move this test alongside it and call `resolve_import` directly
// once it becomes pub.
#[test]
fn test_resolve_import_last_def_wins() {
    // If the target file defines the same name twice, resolve_import
    // should navigate to the last definition.
    let helpers_url = file_url("helpers.R");
    let script_url = file_url("script.R");

    let db = TestDb {
        library: empty_library(),
        sources: HashMap::from([(
            helpers_url.clone(),
            "foo <- function() 'first'\nfoo <- function() 'second'\n".to_string(),
        )]),
    };

    let script_source = "source(\"helpers.R\")\nfoo\n";
    let parsed = parse(script_source, RParserOptions::default());
    let script_root = parsed.syntax();
    let script_idx = semantic_index_with_source_resolver(&parsed.tree(), &script_url, |_path| {
        Some(SourceResolution {
            file: helpers_url.clone(),
            names: vec!["foo".to_string()],
            packages: Vec::new(),
        })
    });

    let dir_layers = script_idx.semantic_calls().to_vec();
    let scope = ExternalScope::search_path(dir_layers, Vec::new());

    let use_offset = script_source.rfind("foo").unwrap() as u32;
    let targets = goto_definition(
        &db,
        offset(use_offset),
        &script_url,
        &script_root,
        &script_idx,
        &scope,
    );

    // Should resolve to the SECOND definition of `foo` in helpers.R (line 1)
    assert_eq!(targets, vec![NavigationTarget {
        file: helpers_url,
        name: "foo".to_string(),
        full_range: text_range(26, 29),
        focus_range: text_range(26, 29),
    }]);
}
