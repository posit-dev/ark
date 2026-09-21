//! Named scenarios with a concrete workspace and edit history.
//!
//! This module registers cases and provides their shared constructors. Source
//! and attachment cases live in `source`, re-export cases in `packages`, and
//! convention-driven project layouts in `layouts`.

mod layouts;
mod packages;
mod source;

use oak_semantic::fuzz::Program;
use oak_semantic::fuzz::Stmt;

use self::layouts::shiny_disabled_autoload_drops_the_r_sibling;
use self::layouts::shiny_entry_sees_global_and_r_files;
use self::layouts::shiny_marker_removed_stops_autoload;
use self::layouts::shiny_nested_app_file_joins_the_enclosing_app;
use self::layouts::testthat_helper_edit_changes_the_test_view;
use self::layouts::testthat_nested_file_is_not_a_testthat_file;
use self::layouts::testthat_setup_outranks_helper;
use self::layouts::testthat_teardown_is_excluded_from_support;
use self::layouts::testthat_test_sees_helpers_and_package;
use self::packages::acyclic_reexport_chain_resolves_to_the_definition;
use self::packages::attached_package_consumer_degrades_on_a_reexport_cycle;
use self::packages::attached_package_consumer_resolves_a_reexport;
use self::packages::mutual_reexport_has_no_terminal_definition;
use self::packages::namespace_import_layer_consumer_resolves_a_reexport;
use self::packages::package_export_shadows_the_source_effect;
use self::packages::reexport_chain_terminates_at_a_local_export;
use self::packages::rename_moves_the_end_of_a_reexport_chain;
use self::source::acyclic_pair_closes_then_reopens;
use self::source::file_or_dir_source_at_a_file;
use self::source::library_in_function_body;
use self::source::mutual_pair_opens_then_closes_again;
use self::source::nested_source_in_function_body;
use self::source::package_cold_entry_reaches_cross_file_layers_recovery;
use self::source::package_edit_revalidates_cross_file_layers_recovery;
use self::source::quote_hole_escapes_source_effect;
use self::source::quote_suppresses_source_effect;
use self::source::recursive_source_dir_in_package;
use self::source::recursive_source_dir_includes_nested;
use self::source::same_file_shadow_suppresses_the_edge;
use self::source::shadow_after_source_call;
use self::source::shallow_source_dir;
use self::source::shallow_source_dir_excludes_nested;
use self::source::source_after_bindings;
use self::source::source_in_eager_block;
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

pub(crate) struct Case {
    pub(crate) name: &'static str,
    pub(crate) scenario: Scenario,
}

pub(crate) fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "acyclic_pair_closes_then_reopens",
            scenario: acyclic_pair_closes_then_reopens(),
        },
        Case {
            name: "testthat_test_sees_helpers_and_package",
            scenario: testthat_test_sees_helpers_and_package(),
        },
        Case {
            name: "testthat_helper_edit_changes_the_test_view",
            scenario: testthat_helper_edit_changes_the_test_view(),
        },
        Case {
            name: "shiny_entry_sees_global_and_r_files",
            scenario: shiny_entry_sees_global_and_r_files(),
        },
        Case {
            name: "shiny_marker_removed_stops_autoload",
            scenario: shiny_marker_removed_stops_autoload(),
        },
        Case {
            name: "shiny_disabled_autoload_drops_the_r_sibling",
            scenario: shiny_disabled_autoload_drops_the_r_sibling(),
        },
        Case {
            name: "shiny_nested_app_file_joins_the_enclosing_app",
            scenario: shiny_nested_app_file_joins_the_enclosing_app(),
        },
        Case {
            name: "testthat_setup_outranks_helper",
            scenario: testthat_setup_outranks_helper(),
        },
        Case {
            name: "testthat_teardown_is_excluded_from_support",
            scenario: testthat_teardown_is_excluded_from_support(),
        },
        Case {
            name: "testthat_nested_file_is_not_a_testthat_file",
            scenario: testthat_nested_file_is_not_a_testthat_file(),
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
            name: "shallow_source_dir_excludes_nested",
            scenario: shallow_source_dir_excludes_nested(),
        },
        Case {
            name: "recursive_source_dir_includes_nested",
            scenario: recursive_source_dir_includes_nested(),
        },
        Case {
            name: "source_in_eager_block",
            scenario: source_in_eager_block(),
        },
        Case {
            name: "quote_suppresses_source_effect",
            scenario: quote_suppresses_source_effect(),
        },
        Case {
            name: "quote_hole_escapes_source_effect",
            scenario: quote_hole_escapes_source_effect(),
        },
        Case {
            name: "shadow_after_source_call",
            scenario: shadow_after_source_call(),
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
            name: "rename_moves_the_end_of_a_reexport_chain",
            scenario: rename_moves_the_end_of_a_reexport_chain(),
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

pub(crate) fn scenario(name: &str) -> Scenario {
    match cases().into_iter().find(|case| case.name == name) {
        Some(case) => case.scenario,
        None => panic!("no corpus case named {name:?}"),
    }
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

/// Uses a caller-specified installed set so qualified effect calls can resolve.
fn scripts_with(installed: &[&str], files: Vec<(&str, Program)>) -> WorkspaceSpec {
    WorkspaceSpec {
        installed: installed.iter().map(|name| name.to_string()).collect(),
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
        let names: Vec<&str> = cases().iter().map(|entry| entry.name).collect();
        let mut deduped = names.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(deduped.len(), names.len());
    }

    #[test]
    fn test_scenario_resolves_every_corpus_name() {
        for entry in cases() {
            scenario(entry.name);
        }
    }
}
