//! Named scenarios with a concrete workspace and edit history.

use oak_semantic::effects::fuzz::SourceProvider;
use oak_semantic::fuzz::Invocation;
use oak_semantic::fuzz::Program;
use oak_semantic::fuzz::Stmt;

use crate::file_imports::CollationView;
use crate::fuzz::build::binding;
use crate::fuzz::build::function_def;
use crate::fuzz::build::library;
use crate::fuzz::build::shadow;
use crate::fuzz::build::source;
use crate::fuzz::build::source_with;
use crate::fuzz::scenario::Edit;
use crate::fuzz::scenario::Op;
use crate::fuzz::scenario::Query;
use crate::fuzz::scenario::Scenario;
use crate::fuzz::spec::FileId;
use crate::fuzz::spec::FileSpec;
use crate::fuzz::spec::Owner;
use crate::fuzz::spec::PackageId;
use crate::fuzz::spec::PackageKind;
use crate::fuzz::spec::PackageSpec;
use crate::fuzz::spec::Reexport;
use crate::fuzz::spec::WorkspaceSpec;
use crate::NamespaceVisibility;

pub struct Case {
    pub name: &'static str,
    pub scenario: Scenario,
}

pub fn corpus() -> Vec<Case> {
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
            name: "package_edit_revalidates_cross_file_layers_recovery",
            scenario: package_edit_revalidates_cross_file_layers_recovery(),
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
        Case {
            name: "acyclic_reexport_chain_resolves_to_the_definition",
            scenario: acyclic_reexport_chain_resolves_to_the_definition(),
        },
        Case {
            name: "mutual_reexport_has_no_terminal_definition",
            scenario: mutual_reexport_has_no_terminal_definition(),
        },
        Case {
            name: "reexport_chain_terminates_at_a_local_export",
            scenario: reexport_chain_terminates_at_a_local_export(),
        },
        Case {
            name: "attached_package_consumer_resolves_a_reexport",
            scenario: attached_package_consumer_resolves_a_reexport(),
        },
        Case {
            name: "attached_package_consumer_degrades_on_a_reexport_cycle",
            scenario: attached_package_consumer_degrades_on_a_reexport_cycle(),
        },
        Case {
            name: "namespace_import_layer_consumer_resolves_a_reexport",
            scenario: namespace_import_layer_consumer_resolves_a_reexport(),
        },
        Case {
            name: "package_export_shadows_the_source_effect",
            scenario: package_export_shadows_the_source_effect(),
        },
    ]
}

pub fn case(name: &str) -> Scenario {
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

/// The cold entry memoizes `cross_file_layers()`, so the edit makes the cycle
/// arise while Salsa revalidates that memo rather than while computing it.
fn package_edit_revalidates_cross_file_layers_recovery() -> Scenario {
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

/// `pkga` has no local definition, so resolution follows its import to `pkgb`.
fn acyclic_reexport_chain_resolves_to_the_definition() -> Scenario {
    let initial = WorkspaceSpec {
        installed: vec!["base".to_string()],
        packages: vec![
            package_spec("pkga", PackageKind::Workspace, &["exp_a"], &[(
                "exp_a", "pkgb",
            )]),
            package_spec("pkgb", PackageKind::Workspace, &["exp_a"], &[]),
        ],
        files: file_specs(Owner::Package(PackageId(1)), vec![(
            "R/a.R",
            program(vec![function_def("exp_a", vec![])]),
        )]),
    };
    scenario(
        initial,
        Query::PackageResolve(
            PackageId(0),
            "exp_a".to_string(),
            NamespaceVisibility::Exported,
        ),
        vec![],
    )
}

/// Neither package defines `exp_a` locally. Following their mutual re-exports
/// re-enters `Package::resolve()` with the same key.
fn mutual_reexport_has_no_terminal_definition() -> Scenario {
    let initial = WorkspaceSpec {
        installed: vec![],
        packages: vec![
            package_spec("lib0", PackageKind::Library, &["exp_a"], &[(
                "exp_a", "lib1",
            )]),
            package_spec("lib1", PackageKind::Library, &["exp_a"], &[(
                "exp_a", "lib0",
            )]),
        ],
        // Library packages own no files. A lone script keeps the workspace
        // non-empty so `validate()` accepts the scenario.
        files: file_specs(Owner::Script, vec![(
            "a.R",
            program(vec![binding("val_a")]),
        )]),
    };
    scenario(
        initial,
        Query::PackageResolve(
            PackageId(0),
            "exp_a".to_string(),
            NamespaceVisibility::Exported,
        ),
        vec![],
    )
}

/// `lib0` re-exports from `lib1`, which re-exports from the workspace package
/// `pkgw`, which defines `exp_a` locally. Separates chain depth (two hops
/// through metadata-only packages) from the mutual-cycle case.
fn reexport_chain_terminates_at_a_local_export() -> Scenario {
    let initial = WorkspaceSpec {
        installed: vec![],
        packages: vec![
            package_spec("lib0", PackageKind::Library, &["exp_a"], &[(
                "exp_a", "lib1",
            )]),
            package_spec("lib1", PackageKind::Library, &["exp_a"], &[(
                "exp_a", "pkgw",
            )]),
            package_spec("pkgw", PackageKind::Workspace, &["exp_a"], &[]),
        ],
        files: file_specs(Owner::Package(PackageId(2)), vec![(
            "R/a.R",
            program(vec![function_def("exp_a", vec![])]),
        )]),
    };
    scenario(
        initial,
        Query::PackageResolve(
            PackageId(0),
            "exp_a".to_string(),
            NamespaceVisibility::Exported,
        ),
        vec![],
    )
}

/// A script attaches `lib0`, which re-exports `exp_a` from the workspace
/// package `pkgw`. `File::resolve()` reaches `Package::resolve()` through the
/// attach's `ImportLayer::Package`, not through a direct `PackageResolve` entry.
fn attached_package_consumer_resolves_a_reexport() -> Scenario {
    let initial = WorkspaceSpec {
        installed: vec!["base".to_string()],
        packages: vec![
            package_spec("lib0", PackageKind::Library, &["exp_a"], &[(
                "exp_a", "pkgw",
            )]),
            package_spec("pkgw", PackageKind::Workspace, &["exp_a"], &[]),
        ],
        files: {
            let mut files = file_specs(Owner::Package(PackageId(1)), vec![(
                "R/a.R",
                program(vec![function_def("exp_a", vec![])]),
            )]);
            files.extend(file_specs(Owner::Script, vec![(
                "a.R",
                program(vec![library("lib0")]),
            )]));
            files
        },
    };
    scenario(
        initial,
        Query::Resolve(FileId(1), "exp_a".to_string()),
        vec![],
    )
}

/// The attaching script enters the mutual re-export cycle through file
/// resolution, exercising recovery from a consumer request.
fn attached_package_consumer_degrades_on_a_reexport_cycle() -> Scenario {
    let initial = WorkspaceSpec {
        installed: vec!["base".to_string()],
        packages: vec![
            package_spec("lib0", PackageKind::Library, &["exp_a"], &[(
                "exp_a", "lib1",
            )]),
            package_spec("lib1", PackageKind::Library, &["exp_a"], &[(
                "exp_a", "lib0",
            )]),
        ],
        files: file_specs(Owner::Script, vec![("a.R", program(vec![library("lib0")]))]),
    };
    scenario(
        initial,
        Query::Resolve(FileId(0), "exp_a".to_string()),
        vec![],
    )
}

/// `pkgc`'s own NAMESPACE carries `importFrom(pkgd, exp_a)` with no matching
/// `export()`, so a file inside `pkgc` sees `exp_a` through
/// `ImportLayer::From`, the collation-wide re-export layer, rather than
/// through an attach.
fn namespace_import_layer_consumer_resolves_a_reexport() -> Scenario {
    let initial = WorkspaceSpec {
        installed: vec!["base".to_string()],
        packages: vec![
            package_spec("pkgc", PackageKind::Workspace, &[], &[("exp_a", "pkgd")]),
            package_spec("pkgd", PackageKind::Workspace, &["exp_a"], &[]),
        ],
        files: {
            let mut files = file_specs(Owner::Package(PackageId(0)), vec![(
                "R/a.R",
                program(vec![binding("val_c")]),
            )]);
            files.extend(file_specs(Owner::Package(PackageId(1)), vec![(
                "R/a.R",
                program(vec![function_def("exp_a", vec![])]),
            )]));
            files
        },
    };
    scenario(
        initial,
        Query::Resolve(FileId(0), "exp_a".to_string()),
        vec![],
    )
}

/// `lib0` exports `source`, so attaching it binds `source` as a plain export
/// (no registered effect) that shadows `base`'s `source()` effect through
/// `package_binding()`, before the search reaches `base`.
fn package_export_shadows_the_source_effect() -> Scenario {
    let initial = WorkspaceSpec {
        installed: vec!["base".to_string()],
        packages: vec![package_spec("lib0", PackageKind::Library, &["source"], &[])],
        files: {
            let mut files = file_specs(Owner::Script, vec![(
                "a.R",
                program(vec![library("lib0"), source("b.R")]),
            )]);
            files.extend(file_specs(Owner::Script, vec![(
                "b.R",
                program(vec![binding("val_b")]),
            )]));
            files
        },
    };
    scenario(initial, Query::Diagnostics(FileId(0)), vec![])
}

fn program(statements: Vec<Stmt>) -> Program {
    Program { statements }
}

/// Include `base` so `source()` can form edges between the scripts.
fn scripts(files: Vec<(&str, Program)>) -> WorkspaceSpec {
    WorkspaceSpec {
        installed: vec!["base".to_string()],
        packages: vec![],
        files: file_specs(Owner::Script, files),
    }
}

fn package(name: &str, installed: &[&str], files: Vec<(&str, Program)>) -> WorkspaceSpec {
    WorkspaceSpec {
        installed: installed.iter().map(|name| name.to_string()).collect(),
        packages: vec![empty_package(name)],
        files: file_specs(Owner::Package(PackageId(0)), files),
    }
}

fn empty_package(name: &str) -> PackageSpec {
    package_spec(name, PackageKind::Workspace, &[], &[])
}

fn package_spec(
    name: &str,
    kind: PackageKind,
    exports: &[&str],
    reexports: &[(&str, &str)],
) -> PackageSpec {
    PackageSpec {
        name: name.to_string(),
        kind,
        exports: exports.iter().map(|export| export.to_string()).collect(),
        reexports: reexports
            .iter()
            .map(|(name, from)| Reexport {
                name: name.to_string(),
                from: from.to_string(),
            })
            .collect(),
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

fn query(query: Query) -> Op {
    Op::Query(query)
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
