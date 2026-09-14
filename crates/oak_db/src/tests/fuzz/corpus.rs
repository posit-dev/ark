//! Named scenarios with a concrete workspace and edit history.

use oak_semantic::effects::fuzz::SourceProvider;
use oak_semantic::fuzz::Invocation;
use oak_semantic::fuzz::Program;
use oak_semantic::fuzz::Stmt;

use crate::file_imports::CollationView;
use crate::tests::fuzz::build::binding;
use crate::tests::fuzz::build::function_def;
use crate::tests::fuzz::build::library;
use crate::tests::fuzz::build::shadow;
use crate::tests::fuzz::build::source;
use crate::tests::fuzz::build::source_with;
use crate::tests::fuzz::scenario::Edit;
use crate::tests::fuzz::scenario::Op;
use crate::tests::fuzz::scenario::Query;
use crate::tests::fuzz::scenario::Scenario;
use crate::tests::fuzz::spec::FileId;
use crate::tests::fuzz::spec::FileSpec;
use crate::tests::fuzz::spec::Owner;
use crate::tests::fuzz::spec::WorkspaceSpec;

pub(super) struct Case {
    pub(super) name: &'static str,
    pub(super) scenario: Scenario,
}

pub(super) fn corpus() -> Vec<Case> {
    vec![
        Case {
            name: "acyclic_pair_closes_then_reopens",
            scenario: acyclic_pair_closes_then_reopens(),
        },
        Case {
            name: "mutual_pair_opens_then_closes_again",
            scenario: mutual_pair_opens_then_closes_again(),
        },
        Case {
            name: "package_cold_entry_reaches_cross_file_layers_recovery",
            scenario: package_cold_entry_reaches_cross_file_layers_recovery(),
        },
        Case {
            name: "same_file_shadow_suppresses_the_edge",
            scenario: same_file_shadow_suppresses_the_edge(),
        },
        Case {
            name: "nested_source_in_function_body",
            scenario: nested_source_in_function_body(),
        },
        Case {
            name: "source_after_bindings",
            scenario: source_after_bindings(),
        },
        Case {
            name: "library_in_function_body",
            scenario: library_in_function_body(),
        },
        Case {
            name: "shallow_source_dir",
            scenario: shallow_source_dir(),
        },
        Case {
            name: "recursive_source_dir_in_package",
            scenario: recursive_source_dir_in_package(),
        },
        Case {
            name: "file_or_dir_source_at_a_file",
            scenario: file_or_dir_source_at_a_file(),
        },
    ]
}

pub(super) fn case(name: &str) -> Scenario {
    match corpus().into_iter().find(|case| case.name == name) {
        Some(case) => case.scenario,
        None => panic!("no corpus case named {name:?}"),
    }
}

fn acyclic_pair_closes_then_reopens() -> Scenario {
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

fn mutual_pair_opens_then_closes_again() -> Scenario {
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

fn package_cold_entry_reaches_cross_file_layers_recovery() -> Scenario {
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

fn same_file_shadow_suppresses_the_edge() -> Scenario {
    let initial = scripts(vec![
        (
            "a.R",
            program(vec![shadow("source"), source("b.R"), binding("val_a")]),
        ),
        ("b.R", program(vec![binding("val_b")])),
    ]);
    scenario(initial, Query::Diagnostics(FileId(0)), vec![])
}

fn nested_source_in_function_body() -> Scenario {
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

fn source_after_bindings() -> Scenario {
    let initial = scripts(vec![
        ("a.R", program(vec![binding("val_a"), source("b.R")])),
        ("b.R", program(vec![binding("val_b")])),
    ]);
    scenario(initial, Query::Diagnostics(FileId(0)), vec![])
}

fn library_in_function_body() -> Scenario {
    let initial = WorkspaceSpec {
        installed: vec!["base".to_string(), "pkga".to_string()],
        package: None,
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

fn shallow_source_dir() -> Scenario {
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
fn recursive_source_dir_in_package() -> Scenario {
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
fn file_or_dir_source_at_a_file() -> Scenario {
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

fn program(statements: Vec<Stmt>) -> Program {
    Program { statements }
}

/// Include `base` so `source()` can form edges between the scripts.
fn scripts(files: Vec<(&str, Program)>) -> WorkspaceSpec {
    WorkspaceSpec {
        installed: vec!["base".to_string()],
        package: None,
        files: file_specs(Owner::Script, files),
    }
}

fn package(name: &str, installed: &[&str], files: Vec<(&str, Program)>) -> WorkspaceSpec {
    WorkspaceSpec {
        installed: installed.iter().map(|name| name.to_string()).collect(),
        package: Some(name.to_string()),
        files: file_specs(Owner::Package, files),
    }
}

fn file_specs(owner: Owner, files: Vec<(&str, Program)>) -> Vec<FileSpec> {
    files
        .into_iter()
        .map(|(path, program)| FileSpec {
            owner,
            path: path.to_string(),
            program,
        })
        .collect()
}

fn scenario(initial: WorkspaceSpec, cold_entry: Query, ops: Vec<Op>) -> Scenario {
    Scenario {
        seed: 0,
        variant: 0,
        initial,
        cold_entry,
        ops,
    }
}

fn replace(file: FileId, program: Program) -> Op {
    Op::Edit(Edit { file, program })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_corpus_names_are_unique() {
        let names: Vec<&str> = corpus().iter().map(|entry| entry.name).collect();
        let mut deduped = names.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(deduped.len(), names.len());
    }

    #[test]
    fn test_case_resolves_every_corpus_name() {
        for entry in corpus() {
            case(entry.name);
        }
    }
}
