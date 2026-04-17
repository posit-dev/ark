use std::path::PathBuf;

use aether_parser::parse;
use aether_parser::RParserOptions;
use assert_matches::assert_matches;
use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_index::builder::build;
use oak_index::external::file_layers;
use oak_index::external::resolve_external_name;
use oak_index::external::BindingSource;
use oak_index::external::ExternalDefinition;
use oak_package::library::Library;
use oak_package::package::Package;
use oak_package::package_description::Description;
use oak_package::package_namespace::Namespace;
use url::Url;

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

fn range(start: u32, end: u32) -> TextRange {
    TextRange::new(TextSize::from(start), TextSize::from(end))
}

fn file_url(name: &str) -> Url {
    Url::parse(&format!("file:///project/R/{name}")).unwrap()
}

fn file_exports(file: &str, entries: Vec<(&str, TextRange)>) -> BindingSource {
    BindingSource::FileExports {
        file: file_url(file),
        exports: entries
            .into_iter()
            .map(|(n, r)| (n.to_string(), r))
            .collect(),
    }
}

fn package_imports(entries: Vec<(&str, &str)>) -> BindingSource {
    BindingSource::PackageImports(
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

    let result = resolve_external_name(&empty_library(), &scope, "helper");
    assert_eq!(
        result,
        Some(ExternalDefinition::ProjectFile {
            file: file_url("utils.R"),
            name: "helper".to_string(),
            range: range(0, 6),
        })
    );
}

#[test]
fn test_resolve_file_exports_miss() {
    let scope = vec![file_exports("utils.R", vec![("helper", range(0, 6))])];

    let result = resolve_external_name(&empty_library(), &scope, "other");
    assert_eq!(result, None);
}

#[test]
fn test_resolve_imported_names() {
    let scope = vec![package_imports(vec![("median", "stats")])];

    let result = resolve_external_name(&empty_library(), &scope, "median");
    assert_eq!(
        result,
        Some(ExternalDefinition::Package {
            package: "stats".to_string(),
            name: "median".to_string(),
        })
    );
}

#[test]
fn test_resolve_package_exports() {
    let library = test_library(vec![("dplyr", vec!["filter", "mutate", "select"])]);

    let scope = vec![BindingSource::PackageExports("dplyr".to_string())];

    let result = resolve_external_name(&library, &scope, "filter");
    assert_eq!(
        result,
        Some(ExternalDefinition::Package {
            package: "dplyr".to_string(),
            name: "filter".to_string(),
        })
    );
}

#[test]
fn test_resolve_package_exports_miss() {
    let library = test_library(vec![("dplyr", vec!["filter", "mutate", "select"])]);

    let scope = vec![BindingSource::PackageExports("dplyr".to_string())];

    let result = resolve_external_name(&library, &scope, "summarise");
    assert_eq!(result, None);
}

#[test]
fn test_resolve_unknown_package_skipped() {
    let scope = vec![BindingSource::PackageExports("nonexistent".to_string())];

    let result = resolve_external_name(&empty_library(), &scope, "foo");
    assert_eq!(result, None);
}

#[test]
fn test_resolve_package_shadowing() {
    // Both dplyr and stats export `filter`. dplyr was loaded later so it
    // appears earlier in the scope and shadows stats's version.
    let library = test_library(vec![
        ("stats", vec!["filter", "median"]),
        ("dplyr", vec!["filter", "mutate"]),
    ]);

    let scope = vec![
        BindingSource::PackageExports("dplyr".to_string()),
        BindingSource::PackageExports("stats".to_string()),
    ];

    // dplyr's `filter` wins
    let result = resolve_external_name(&library, &scope, "filter");
    assert_eq!(
        result,
        Some(ExternalDefinition::Package {
            package: "dplyr".to_string(),
            name: "filter".to_string(),
        })
    );

    // `median` only in stats, falls through
    let result = resolve_external_name(&library, &scope, "median");
    assert_eq!(
        result,
        Some(ExternalDefinition::Package {
            package: "stats".to_string(),
            name: "median".to_string(),
        })
    );
}

#[test]
fn test_resolve_first_match_wins() {
    let library = test_library(vec![("stats", vec!["filter"])]);

    let scope = vec![
        file_exports("utils.R", vec![("filter", range(0, 6))]),
        BindingSource::PackageExports("stats".to_string()),
    ];

    // File export should win over package export
    let result = resolve_external_name(&library, &scope, "filter");
    assert_eq!(
        result,
        Some(ExternalDefinition::ProjectFile {
            file: file_url("utils.R"),
            name: "filter".to_string(),
            range: range(0, 6),
        })
    );
}

#[test]
fn test_resolve_falls_through_to_later_layer() {
    let library = test_library(vec![("dplyr", vec!["filter", "mutate"])]);

    let scope = vec![
        file_exports("utils.R", vec![("helper", range(0, 6))]),
        BindingSource::PackageExports("dplyr".to_string()),
    ];

    // "filter" is not in file exports, falls through to package
    let result = resolve_external_name(&library, &scope, "filter");
    assert_eq!(
        result,
        Some(ExternalDefinition::Package {
            package: "dplyr".to_string(),
            name: "filter".to_string(),
        })
    );
}

#[test]
fn test_resolve_imported_names_shadow_package_exports() {
    let library = test_library(vec![("dplyr", vec!["filter"])]);

    let scope = vec![
        package_imports(vec![("filter", "stats")]),
        BindingSource::PackageExports("dplyr".to_string()),
    ];

    let result = resolve_external_name(&library, &scope, "filter");
    assert_eq!(
        result,
        Some(ExternalDefinition::Package {
            package: "stats".to_string(),
            name: "filter".to_string(),
        })
    );
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

    let result = resolve_external_name(&empty_library(), &scope, "x");
    assert_eq!(
        result,
        Some(ExternalDefinition::ProjectFile {
            file: file_url("utils.R"),
            name: "x".to_string(),
            range: range(10, 11),
        })
    );
}

// --- file_layers ---

fn index_source(source: &str) -> oak_index::semantic_index::SemanticIndex {
    let parsed = parse(source, RParserOptions::default());
    build(&parsed.tree())
}

#[test]
fn test_file_layers_exports_only() {
    let index = index_source("x <- 1\ny <- 2");
    let layers = file_layers(file_url("foo.R"), &index);

    assert_eq!(layers.len(), 1);
    assert_matches!(&layers[0], BindingSource::FileExports { file, exports } => {
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

    assert_matches!(&layers[0], BindingSource::FileExports { exports, .. } => {
        assert_eq!(exports.len(), 1);
        assert!(exports.contains_key("x"));
    });
    assert_matches!(&layers[1], BindingSource::PackageExports(pkg) => {
        assert_eq!(pkg, "dplyr");
    });
    assert_matches!(&layers[2], BindingSource::PackageExports(pkg) => {
        assert_eq!(pkg, "tidyr");
    });
}

#[test]
fn test_file_layers_last_def_wins() {
    // "x <- 1\nx <- 2" has two definitions of x; the map should keep the last
    let index = index_source("x <- 1\nx <- 2");
    let layers = file_layers(file_url("foo.R"), &index);

    assert_matches!(&layers[0], BindingSource::FileExports { exports, .. } => {
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
    assert_matches!(&layers[0], BindingSource::FileExports { exports, .. } => {
        assert!(exports.is_empty());
    });
}

// --- Integration: file_layers -> resolve_external_name ---

#[test]
fn test_file_layers_resolve_roundtrip() {
    let library = test_library(vec![("dplyr", vec!["filter", "mutate"])]);

    let index = index_source("library(dplyr)\nmy_helper <- function() NULL");
    let layers = file_layers(file_url("script.R"), &index);

    // Resolve a file export
    let result = resolve_external_name(&library, &layers, "my_helper");
    assert!(matches!(
        result,
        Some(ExternalDefinition::ProjectFile { .. })
    ));

    // Resolve a package export
    let result = resolve_external_name(&library, &layers, "filter");
    assert_eq!(
        result,
        Some(ExternalDefinition::Package {
            package: "dplyr".to_string(),
            name: "filter".to_string(),
        })
    );

    // Miss
    let result = resolve_external_name(&library, &layers, "unknown");
    assert_eq!(result, None);
}

#[test]
fn test_chained_scope_predecessor_files() {
    let library = test_library(vec![("ggplot2", vec!["aes", "ggplot"])]);

    // Simulate two predecessor files, then root layers
    let index_a = index_source("helper_a <- 1");
    let index_b = index_source("library(ggplot2)\nhelper_b <- 2");

    let mut scope = Vec::new();

    // Later files come first (they shadow earlier ones)
    scope.extend(file_layers(file_url("b.R"), &index_b));
    scope.extend(file_layers(file_url("a.R"), &index_a));

    // Resolve from predecessor file b
    let result = resolve_external_name(&library, &scope, "helper_b");
    assert!(matches!(
        result,
        Some(ExternalDefinition::ProjectFile { .. })
    ));

    // Resolve from predecessor file a
    let result = resolve_external_name(&library, &scope, "helper_a");
    assert!(matches!(
        result,
        Some(ExternalDefinition::ProjectFile { .. })
    ));

    // Resolve from ggplot2 (attached by b.R)
    let result = resolve_external_name(&library, &scope, "ggplot");
    assert_eq!(
        result,
        Some(ExternalDefinition::Package {
            package: "ggplot2".to_string(),
            name: "ggplot".to_string(),
        })
    );
}
