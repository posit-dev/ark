use std::path::PathBuf;
use std::sync::Arc;

use aether_parser::parse;
use aether_parser::RParserOptions;
use assert_matches::assert_matches;
use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_package_metadata::description::Description;
use oak_package_metadata::namespace::Import;
use oak_package_metadata::namespace::Namespace;
use oak_semantic::build_index;
use oak_semantic::external::resolve_external_name;
use oak_semantic::library::Library;
use oak_semantic::package::Package;
use oak_semantic::scope_layer::file_layers;
use oak_semantic::scope_layer::package_root_layers;
use oak_semantic::scope_layer::ScopeLayer;
use oak_semantic::NoopResolver;
use oak_sources::test::TestPackageCache;
use stdext::SortedVec;
use url::Url;

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

fn range(start: u32, end: u32) -> TextRange {
    TextRange::new(TextSize::from(start), TextSize::from(end))
}

fn file_url(name: &str) -> Url {
    Url::parse(&format!("file:///project/R/{name}")).unwrap()
}

fn file_exports(file: &str, entries: Vec<(&str, TextRange)>) -> ScopeLayer {
    ScopeLayer::FileExports {
        file: file_url(file),
        exports: entries
            .into_iter()
            .map(|(n, r)| (n.to_string(), r))
            .collect(),
    }
}

fn package_imports(entries: Vec<(&str, &str)>) -> ScopeLayer {
    ScopeLayer::PackageImports(
        entries
            .into_iter()
            .map(|(n, p)| (n.to_string(), p.to_string()))
            .collect(),
    )
}

// --- resolve_external_name ---

#[test]
fn test_resolve_file_exports() {
    let scope = vec![file_exports("utils.R", vec![("helper", range(0, 6))])];

    let result = resolve_external_name(&empty_library(), &scope, "helper").unwrap();
    assert_eq!(result.file(), &file_url("utils.R"));
    assert_eq!(result.name(), "helper");
    assert_eq!(result.range(), range(0, 6));
}

#[test]
fn test_resolve_file_exports_miss() {
    let scope = vec![file_exports("utils.R", vec![("helper", range(0, 6))])];

    let result = resolve_external_name(&empty_library(), &scope, "other");
    assert_eq!(result, None);
}

#[test]
fn test_resolve_imported_names() {
    let library = test_library(vec![TestPackage::new(
        "stats",
        "stats.R",
        vec!["median"],
        vec![],
    )]);
    let scope = vec![package_imports(vec![("median", "stats")])];

    let result = resolve_external_name(&library, &scope, "median").unwrap();
    assert!(result.file().path().ends_with("stats.R"));
    assert_eq!(result.name(), "median")
}

#[test]
fn test_resolve_package_exports() {
    let library = test_library(vec![TestPackage::new(
        "dplyr",
        "dplyr.R",
        vec!["filter", "mutate", "select"],
        vec![],
    )]);

    let scope = vec![ScopeLayer::PackageExports("dplyr".to_string())];

    let result = resolve_external_name(&library, &scope, "filter").unwrap();
    assert!(result.file().path().ends_with("dplyr.R"));
    assert_eq!(result.name(), "filter");
}

#[test]
fn test_resolve_package_exports_miss() {
    let library = test_library(vec![TestPackage::new(
        "dplyr",
        "dplyr.R",
        vec!["filter", "mutate", "select"],
        vec![],
    )]);

    let scope = vec![ScopeLayer::PackageExports("dplyr".to_string())];

    let result = resolve_external_name(&library, &scope, "summarise");
    assert_eq!(result, None);
}

#[test]
fn test_resolve_unknown_package_skipped() {
    let scope = vec![ScopeLayer::PackageExports("nonexistent".to_string())];

    let result = resolve_external_name(&empty_library(), &scope, "foo");
    assert_eq!(result, None);
}

#[test]
fn test_resolve_package_shadowing() {
    // Both dplyr and stats export `filter`. dplyr was loaded later so it
    // appears earlier in the scope and shadows stats's version.
    let library = test_library(vec![
        TestPackage::new("stats", "stats.R", vec!["filter", "median"], vec![]),
        TestPackage::new("dplyr", "dplyr.R", vec!["filter", "mutate"], vec![]),
    ]);

    let scope = vec![
        ScopeLayer::PackageExports("dplyr".to_string()),
        ScopeLayer::PackageExports("stats".to_string()),
    ];

    // dplyr's `filter` wins
    let result = resolve_external_name(&library, &scope, "filter").unwrap();
    assert!(result.file().path().ends_with("dplyr.R"));
    assert_eq!(result.name(), "filter");

    // `median` only in stats, falls through
    let result = resolve_external_name(&library, &scope, "median").unwrap();
    assert!(result.file().path().ends_with("stats.R"));
    assert_eq!(result.name(), "median");
}

#[test]
fn test_resolve_first_match_wins() {
    let library = test_library(vec![TestPackage::new(
        "stats",
        "stats.R",
        vec!["filter"],
        vec![],
    )]);

    let scope = vec![
        file_exports("utils.R", vec![("filter", range(0, 6))]),
        ScopeLayer::PackageExports("stats".to_string()),
    ];

    // File export should win over package export
    let result = resolve_external_name(&library, &scope, "filter").unwrap();
    assert_eq!(result.file(), &file_url("utils.R"));
    assert_eq!(result.name(), "filter");
    assert_eq!(result.range(), range(0, 6));
}

#[test]
fn test_resolve_falls_through_to_later_layer() {
    let library = test_library(vec![TestPackage::new(
        "dplyr",
        "dplyr.R",
        vec!["filter", "mutate"],
        vec![],
    )]);

    let scope = vec![
        file_exports("utils.R", vec![("helper", range(0, 6))]),
        ScopeLayer::PackageExports("dplyr".to_string()),
    ];

    // "filter" is not in file exports, falls through to package
    let result = resolve_external_name(&library, &scope, "filter").unwrap();
    assert!(result.file().path().ends_with("dplyr.R"));
    assert_eq!(result.name(), "filter");
}

#[test]
fn test_resolve_imported_names_shadow_package_exports() {
    let library = test_library(vec![
        TestPackage::new("dplyr", "dplyr.R", vec!["filter"], vec![]),
        TestPackage::new("stats", "stats.R", vec!["filter"], vec![]),
    ]);

    let scope = vec![
        package_imports(vec![("filter", "stats")]),
        ScopeLayer::PackageExports("dplyr".to_string()),
    ];

    let result = resolve_external_name(&library, &scope, "filter").unwrap();
    assert!(result.file().path().ends_with("stats.R"));
    assert_eq!(result.name(), "filter");
}

#[test]
fn test_resolve_empty_scope() {
    let result = resolve_external_name(&empty_library(), &[], "anything");
    assert_eq!(result, None);
}

#[test]
fn test_resolve_file_exports_last_definition_wins() {
    // HashMap built from a vec: last insert wins
    let scope = vec![file_exports("utils.R", vec![
        ("x", range(0, 1)),
        ("x", range(10, 11)),
    ])];

    let result = resolve_external_name(&empty_library(), &scope, "x").unwrap();
    assert_eq!(result.file(), &file_url("utils.R"));
    assert_eq!(result.name(), "x");
    assert_eq!(result.range(), range(10, 11));
}

// --- file_layers ---

fn index_source(source: &str) -> oak_semantic::semantic_index::SemanticIndex {
    let parsed = parse(source, RParserOptions::default());
    build_index(
        &parsed.tree(),
        &url::Url::parse("file:///test/test.R").unwrap(),
        &mut NoopResolver,
    )
}

#[test]
fn test_file_layers_exports_only() {
    let index = index_source("x <- 1\ny <- 2");
    let layers = file_layers(file_url("foo.R"), &index);

    assert_eq!(layers.len(), 1);
    assert_matches!(&layers[0], ScopeLayer::FileExports { file, exports } => {
        assert_eq!(file, &file_url("foo.R"));
        assert!(exports.contains_key("x"));
        assert!(exports.contains_key("y"));
        assert_eq!(exports.len(), 2);
    });
}

#[test]
fn test_file_layers_with_library_directives() {
    let index = index_source("library(dplyr)\nlibrary(tidyr)\nx <- 1");
    let layers = file_layers(file_url("script.R"), &index);

    // FileExports + 2 PackageExports
    assert_eq!(layers.len(), 3);

    assert_matches!(&layers[0], ScopeLayer::FileExports { exports, .. } => {
        assert_eq!(exports.len(), 1);
        assert!(exports.contains_key("x"));
    });
    assert_matches!(&layers[1], ScopeLayer::PackageExports(pkg) => {
        assert_eq!(pkg, "dplyr");
    });
    assert_matches!(&layers[2], ScopeLayer::PackageExports(pkg) => {
        assert_eq!(pkg, "tidyr");
    });
}

#[test]
fn test_file_layers_last_def_wins() {
    // "x <- 1\nx <- 2" has two definitions of x; the map should keep the last
    let index = index_source("x <- 1\nx <- 2");
    let layers = file_layers(file_url("foo.R"), &index);

    assert_matches!(&layers[0], ScopeLayer::FileExports { exports, .. } => {
        assert_eq!(exports.len(), 1);
        let range = exports.get("x").unwrap();
        assert_eq!(range.start(), TextSize::from(7));
    });
}

#[test]
fn test_file_layers_empty_file() {
    let index = index_source("");
    let layers = file_layers(file_url("empty.R"), &index);

    assert_eq!(layers.len(), 1);
    assert_matches!(&layers[0], ScopeLayer::FileExports { exports, .. } => {
        assert!(exports.is_empty());
    });
}

#[test]
fn test_file_layers_source_directive_skipped() {
    let index = index_source("library(dplyr)\nsource(\"helpers.R\")\nx <- 1");
    let layers = file_layers(file_url("script.R"), &index);

    // FileExports + PackageExports(dplyr), source() is not emitted as a layer
    assert_eq!(layers.len(), 2);
    assert_matches!(&layers[0], ScopeLayer::FileExports { exports, .. } => {
        assert_eq!(exports.len(), 1);
        assert!(exports.contains_key("x"));
    });
    assert_matches!(&layers[1], ScopeLayer::PackageExports(pkg) => {
        assert_eq!(pkg, "dplyr");
    });
}

// --- Integration: file_layers -> resolve_external_name ---

#[test]
fn test_file_layers_resolve_roundtrip() {
    let library = test_library(vec![TestPackage::new(
        "dplyr",
        "dplyr.R",
        vec!["filter", "mutate"],
        vec![],
    )]);

    let index = index_source("library(dplyr)\nmy_helper <- function() NULL");
    let layers = file_layers(file_url("script.R"), &index);

    // Resolve a file export
    let result = resolve_external_name(&library, &layers, "my_helper").unwrap();
    assert_eq!(result.file(), &file_url("script.R"));

    // Resolve a package export
    let result = resolve_external_name(&library, &layers, "filter").unwrap();
    assert!(result.file().path().ends_with("dplyr.R"));
    assert_eq!(result.name(), "filter");

    // Miss
    let result = resolve_external_name(&library, &layers, "unknown");
    assert_eq!(result, None);
}

#[test]
fn test_chained_scope_predecessor_files() {
    let library = test_library(vec![TestPackage::new(
        "ggplot2",
        "ggplot2.R",
        vec!["aes", "ggplot"],
        vec![],
    )]);

    // Simulate two predecessor files, then root layers
    let index_a = index_source("helper_a <- 1");
    let index_b = index_source("library(ggplot2)\nhelper_b <- 2");

    let mut scope = Vec::new();

    // Later files come first (they shadow earlier ones)
    scope.extend(file_layers(file_url("b.R"), &index_b));
    scope.extend(file_layers(file_url("a.R"), &index_a));

    // Resolve from predecessor file b
    // Predecessor file export from b.R
    let result = resolve_external_name(&library, &scope, "helper_b").unwrap();
    assert_eq!(result.file(), &file_url("b.R"));

    // Resolve from predecessor file a
    let result = resolve_external_name(&library, &scope, "helper_a").unwrap();
    assert_eq!(result.file(), &file_url("a.R"));

    // Resolve from ggplot2 (attached by b.R)
    let result = resolve_external_name(&library, &scope, "ggplot").unwrap();
    assert!(result.file().path().ends_with("ggplot2.R"));
    assert_eq!(result.name(), "ggplot");
}

// --- root_layers ---

#[test]
fn test_root_layers_from_namespace_imports() {
    let ns = Namespace {
        package_imports: vec!["rlang".to_string(), "cli".to_string()],
        ..Default::default()
    };
    let layers = package_root_layers(&ns);
    assert_eq!(layers.len(), 3);
    assert_matches!(&layers[0], ScopeLayer::PackageExports(pkg) => {
        assert_eq!(pkg, "rlang");
    });
    assert_matches!(&layers[1], ScopeLayer::PackageExports(pkg) => {
        assert_eq!(pkg, "cli");
    });
    assert_matches!(&layers[2], ScopeLayer::PackageExports(pkg) => {
        assert_eq!(pkg, "base");
    });
}

#[test]
fn test_root_layers_empty_namespace() {
    let ns = Namespace::default();
    let layers = package_root_layers(&ns);
    assert_eq!(layers.len(), 1);
    assert_matches!(&layers[0], ScopeLayer::PackageExports(pkg) => {
        assert_eq!(pkg, "base");
    });
}

#[test]
fn test_root_layers_includes_importfrom() {
    let ns = Namespace {
        imports: vec![
            Import {
                name: "median".to_string(),
                package: "stats".to_string(),
            },
            Import {
                name: "head".to_string(),
                package: "utils".to_string(),
            },
        ],
        ..Default::default()
    };
    let layers = package_root_layers(&ns);
    assert_eq!(layers.len(), 2);
    assert_matches!(&layers[0], ScopeLayer::PackageImports(map) => {
        assert_eq!(map.get("median").unwrap(), "stats");
        assert_eq!(map.get("head").unwrap(), "utils");
    });
    assert_matches!(&layers[1], ScopeLayer::PackageExports(pkg) => {
        assert_eq!(pkg, "base");
    });
}

#[test]
fn test_root_layers_importfrom_before_package_exports() {
    let ns = Namespace {
        imports: vec![Import {
            name: "filter".to_string(),
            package: "stats".to_string(),
        }],
        package_imports: vec!["dplyr".to_string()],
        ..Default::default()
    };
    let layers = package_root_layers(&ns);
    assert_eq!(layers.len(), 3);
    assert_matches!(&layers[0], ScopeLayer::PackageImports(_));
    assert_matches!(&layers[1], ScopeLayer::PackageExports(pkg) => {
        assert_eq!(pkg, "dplyr");
    });
    assert_matches!(&layers[2], ScopeLayer::PackageExports(pkg) => {
        assert_eq!(pkg, "base");
    });
}

// --- scope chain assembly ---

#[test]
fn test_scope_chain_combines_predecessors_and_root() {
    let library = test_library(vec![
        TestPackage::new("rlang", "rlang.R", vec!["sym", "expr"], vec![]),
        TestPackage::new("dplyr", "dplyr.R", vec!["filter", "mutate"], vec![]),
    ]);

    let index_a = index_source("helper_a <- 1");
    let index_b = index_source("library(dplyr)\nhelper_b <- 2");

    let ns = Namespace {
        package_imports: vec!["rlang".to_string()],
        ..Default::default()
    };

    let mut scope = Vec::new();
    scope.extend(file_layers(file_url("b.R"), &index_b));
    scope.extend(file_layers(file_url("a.R"), &index_a));
    scope.extend(package_root_layers(&ns));

    // Predecessor file export
    let result = resolve_external_name(&library, &scope, "helper_b").unwrap();
    assert_eq!(result.file(), &file_url("b.R"));
    assert_eq!(result.name(), "helper_b");
    assert_eq!(result.range(), range(15, 23));

    // Predecessor library() directive
    let result = resolve_external_name(&library, &scope, "filter").unwrap();
    assert!(result.file().path().ends_with("dplyr.R"));
    assert_eq!(result.name(), "filter");

    // Root layer (NAMESPACE import)
    let result = resolve_external_name(&library, &scope, "sym").unwrap();
    assert!(result.file().path().ends_with("rlang.R"));
    assert_eq!(result.name(), "sym");

    // Miss
    assert_eq!(resolve_external_name(&library, &scope, "unknown"), None);
}

#[test]
fn test_scope_chain_predecessors_shadow_root() {
    let library = test_library(vec![TestPackage::new(
        "rlang",
        "rlang.R",
        vec!["expr"],
        vec![],
    )]);

    let index = index_source("expr <- function() NULL");

    let ns = Namespace {
        package_imports: vec!["rlang".to_string()],
        ..Default::default()
    };

    let mut scope = Vec::new();
    scope.extend(file_layers(file_url("utils.R"), &index));
    scope.extend(package_root_layers(&ns));

    // File export shadows the rlang root layer
    let result = resolve_external_name(&library, &scope, "expr").unwrap();
    assert_eq!(result.file(), &file_url("utils.R"));
}

#[test]
fn test_scope_chain_later_predecessor_shadows_earlier() {
    // b.R is loaded after a.R, so b.R's layers come first in the scope
    // and its `helper` should shadow a.R's `helper`.
    let index_a = index_source("helper <- 1");
    let index_b = index_source("helper <- 2");

    let mut scope = Vec::new();
    scope.extend(file_layers(file_url("b.R"), &index_b));
    scope.extend(file_layers(file_url("a.R"), &index_a));

    let result = resolve_external_name(&empty_library(), &scope, "helper").unwrap();
    assert_eq!(result.file(), &file_url("b.R"));
}
