//! Named source-call and attachment scenarios.

use oak_semantic::effects::fuzz::SourceProvider;
use oak_semantic::fuzz::Invocation;

use super::file_specs;
use super::package;
use super::program;
use super::query;
use super::replace;
use super::scenario;
use super::scripts;
use crate::file_imports::CollationView;
use crate::fuzz::build::binding;
use crate::fuzz::build::function_def;
use crate::fuzz::build::library;
use crate::fuzz::build::shadow;
use crate::fuzz::build::source;
use crate::fuzz::build::source_with;
use crate::fuzz::scenario::Query;
use crate::fuzz::scenario::Scenario;
use crate::fuzz::spec::FileId;
use crate::fuzz::spec::Owner;
use crate::fuzz::spec::WorkspaceSpec;

pub(super) fn acyclic_pair_closes_then_reopens() -> Scenario {
    let initial = scripts(vec![
        ("a.R", program(vec![source("b.R"), binding("val_a")])),
        ("b.R", program(vec![binding("val_b")])),
    ]);
    let ops = vec![
        replace(FileId(1), program(vec![source("a.R"), binding("val_b")])),
        replace(FileId(1), program(vec![binding("val_b")])),
    ];
    scenario(initial, Query::Diagnostics(FileId(0)), ops)
}

pub(super) fn mutual_pair_opens_then_closes_again() -> Scenario {
    let initial = scripts(vec![
        ("a.R", program(vec![source("b.R"), binding("val_a")])),
        ("b.R", program(vec![source("a.R"), binding("val_b")])),
    ]);
    let ops = vec![
        replace(FileId(1), program(vec![binding("val_b")])),
        replace(FileId(1), program(vec![source("a.R"), binding("val_b")])),
    ];
    scenario(initial, Query::Diagnostics(FileId(1)), ops)
}

pub(super) fn package_cold_entry_reaches_cross_file_layers_recovery() -> Scenario {
    let initial = package("mypkg", &["base", "pkga"], vec![
        ("R/a.R", program(vec![library("pkga"), source("R/b.R")])),
        ("R/b.R", program(vec![library("pkga")])),
    ]);
    scenario(
        initial,
        Query::CrossFileLayers(FileId(1), CollationView::Eager),
        vec![],
    )
}

/// The cold entry memoizes `cross_file_layers()`, so the edit makes the cycle
/// arise while Salsa revalidates that memo rather than while computing it.
pub(super) fn package_edit_revalidates_cross_file_layers_recovery() -> Scenario {
    let initial = package("mypkg", &["base", "pkga"], vec![
        ("R/a.R", program(vec![])),
        ("R/b.R", program(vec![library("pkga")])),
    ]);
    let ops = vec![
        replace(FileId(0), program(vec![source("R/b.R")])),
        query(Query::CrossFileLayers(FileId(1), CollationView::Eager)),
    ];
    scenario(initial, Query::Imports(FileId(1)), ops)
}

pub(super) fn same_file_shadow_suppresses_the_edge() -> Scenario {
    let initial = scripts(vec![
        (
            "a.R",
            program(vec![shadow("source"), source("b.R"), binding("val_a")]),
        ),
        ("b.R", program(vec![binding("val_b")])),
    ]);
    scenario(initial, Query::Diagnostics(FileId(0)), vec![])
}

pub(super) fn nested_source_in_function_body() -> Scenario {
    let initial = scripts(vec![
        (
            "a.R",
            program(vec![
                function_def("read", vec![source("b.R")]),
                binding("val_a"),
            ]),
        ),
        ("b.R", program(vec![binding("val_b")])),
    ]);
    scenario(initial, Query::Diagnostics(FileId(0)), vec![])
}

pub(super) fn source_after_bindings() -> Scenario {
    let initial = scripts(vec![
        ("a.R", program(vec![binding("val_a"), source("b.R")])),
        ("b.R", program(vec![binding("val_b")])),
    ]);
    scenario(initial, Query::Diagnostics(FileId(0)), vec![])
}

pub(super) fn library_in_function_body() -> Scenario {
    let initial = WorkspaceSpec {
        installed: vec!["base".to_string(), "pkga".to_string()],
        packages: vec![],
        files: file_specs(Owner::Script, vec![(
            "a.R",
            program(vec![
                function_def("attach", vec![library("pkga")]),
                binding("val_a"),
            ]),
        )]),
    };
    scenario(initial, Query::AttachedPackages(FileId(0)), vec![])
}

pub(super) fn shallow_source_dir() -> Scenario {
    let initial = scripts(vec![
        (
            "a.R",
            program(vec![source_with(
                ".",
                SourceProvider::Dir,
                Invocation::Bare,
            )]),
        ),
        ("b.R", program(vec![binding("val_b")])),
    ]);
    scenario(initial, Query::Diagnostics(FileId(0)), vec![])
}

/// Qualify `tar_source()` so it resolves without attaching `targets`.
pub(super) fn recursive_source_dir_in_package() -> Scenario {
    let initial = package("mypkg", &["base", "targets"], vec![
        (
            "R/a.R",
            program(vec![source_with(
                "R",
                SourceProvider::FileOrDir,
                Invocation::Qualified,
            )]),
        ),
        ("R/b.R", program(vec![binding("val_b")])),
    ]);
    scenario(initial, Query::Diagnostics(FileId(0)), vec![])
}

/// `tar_source()` takes a file or a directory, and `scan_source()` tries
/// `resolve_source()` before falling back to the directory walk.
pub(super) fn file_or_dir_source_at_a_file() -> Scenario {
    let initial = package("mypkg", &["base", "targets"], vec![
        (
            "R/a.R",
            program(vec![source_with(
                "R/b.R",
                SourceProvider::FileOrDir,
                Invocation::Qualified,
            )]),
        ),
        ("R/b.R", program(vec![binding("val_b")])),
    ]);
    scenario(initial, Query::Diagnostics(FileId(0)), vec![])
}
