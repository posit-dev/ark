//! Behavior checks on named corpus cases.
//!
//! These assert what the queries do with a concrete workspace and edit history:
//! which `source()` calls form edges, how attaches are classified, and which
//! queries recover on a cycle. The corpus supplies the fixtures. The property
//! blocks in the parent module cover mutated scenarios instead.

use crate::fuzz::corpus;
use crate::fuzz::start;
use crate::fuzz::FileId;
use crate::fuzz::PackageId;
use crate::fuzz::Unobserved;
use crate::recovery;
use crate::NamespaceVisibility;

const NO_TARGETS: [&str; 0] = [];
const NO_PACKAGES: [&str; 0] = [];

#[test]
fn test_scenario_acyclic_pair_closes_then_reopens() {
    let scenario = corpus::scenario("acyclic_pair_closes_then_reopens");
    let mut world = start(&scenario);

    assert_eq!(world.source_targets(FileId(0)), ["w/b.R"]);
    assert_eq!(world.source_targets(FileId(1)), NO_TARGETS);
    assert!(!world.any_source_cycle());

    world.apply(&scenario.ops[0], &mut Unobserved);

    assert!(world.source_cycle_reported(FileId(0)));
    assert!(world.source_cycle_reported(FileId(1)));
    // `NoopImportsResolver` resolves no `source()` callee, so recovery removes
    // the cycle-forming edges from the reporting index.
    assert_eq!(world.source_targets(FileId(0)), NO_TARGETS);

    world.apply(&scenario.ops[1], &mut Unobserved);

    assert!(!world.any_source_cycle());
    assert_eq!(world.source_targets(FileId(0)), ["w/b.R"]);
}

#[test]
fn test_scenario_rename_and_undo_across_files() {
    let scenario = corpus::scenario("rename_and_undo_across_files");
    let mut world = start(&scenario);

    world.apply(&scenario.ops[0], &mut Unobserved);

    assert_eq!(world.file_resolve(FileId(1), "val_0"), ["w/a.R"]);
    assert_eq!(world.file_resolve(FileId(1), "val_3"), NO_TARGETS);

    world.apply(&scenario.ops[1], &mut Unobserved);
    world.apply(&scenario.ops[2], &mut Unobserved);
    world.apply(&scenario.ops[3], &mut Unobserved);

    assert_eq!(world.file_resolve(FileId(1), "val_0"), NO_TARGETS);
    assert_eq!(world.file_resolve(FileId(1), "val_3"), ["w/a.R"]);

    world.apply(&scenario.ops[4], &mut Unobserved);
    world.apply(&scenario.ops[5], &mut Unobserved);
    world.apply(&scenario.ops[6], &mut Unobserved);

    assert_eq!(world.file_resolve(FileId(1), "val_0"), ["w/a.R"]);
    assert_eq!(world.file_resolve(FileId(1), "val_3"), NO_TARGETS);
}

#[test]
fn test_scenario_mutual_pair_opens_then_closes_again() {
    let scenario = corpus::scenario("mutual_pair_opens_then_closes_again");
    let mut world = start(&scenario);

    assert!(world.source_cycle_reported(FileId(0)));
    assert!(world.source_cycle_reported(FileId(1)));

    world.apply(&scenario.ops[0], &mut Unobserved);

    assert!(!world.any_source_cycle());
    assert_eq!(world.source_targets(FileId(0)), ["w/b.R"]);

    world.apply(&scenario.ops[1], &mut Unobserved);

    assert!(world.source_cycle_reported(FileId(0)));
    assert!(world.source_cycle_reported(FileId(1)));
}

/// Require `cross_file_layers()` to become Salsa's repeated key when entered
/// cold through a package with an `R/` collation.
#[test]
fn test_scenario_package_cold_entry_reaches_cross_file_layers_recovery() {
    let scenario = corpus::scenario("package_cold_entry_reaches_cross_file_layers_recovery");
    let _world = start(&scenario);

    let mut fired = recovery::fired();
    fired.sort();
    fired.dedup();
    assert_eq!(fired, [
        "attached_packages(p/mypkg/R/a.R)",
        "attached_packages(p/mypkg/R/b.R)",
        "cross_file_layers(p/mypkg/R/b.R, Eager)",
        "exports(p/mypkg/R/a.R)",
        "exports(p/mypkg/R/b.R)",
        "semantic_index(p/mypkg/R/a.R)",
        "semantic_index(p/mypkg/R/b.R)",
    ]);
}

/// A memoized cycle result does not survive a revision bump, so revalidating
/// `cross_file_layers()` after the edit consults its handler again.
#[test]
fn test_scenario_package_edit_revalidates_cross_file_layers_recovery() {
    let scenario = corpus::scenario("package_edit_revalidates_cross_file_layers_recovery");
    let mut world = start(&scenario);

    // Attribute only the post-edit firings, since the cold entry recovers too.
    recovery::reset();
    for op in &scenario.ops {
        world.apply(op, &mut Unobserved);
    }

    let mut fired = recovery::fired();
    fired.sort();
    fired.dedup();
    assert_eq!(fired, [
        "attached_packages(p/mypkg/R/a.R)",
        "attached_packages(p/mypkg/R/b.R)",
        "cross_file_layers(p/mypkg/R/b.R, Eager)",
        "exports(p/mypkg/R/a.R)",
        "exports(p/mypkg/R/b.R)",
        "semantic_index(p/mypkg/R/a.R)",
        "semantic_index(p/mypkg/R/b.R)",
    ]);
}

/// Verify that a locally shadowed `source()` contributes no source edge.
#[test]
fn test_scenario_same_file_shadow_suppresses_the_edge() {
    let scenario = corpus::scenario("same_file_shadow_suppresses_the_edge");
    let world = start(&scenario);

    assert_eq!(world.source_targets(FileId(0)), NO_TARGETS);
}

/// A nested `source()` forms an edge even when the function is never called.
#[test]
fn test_scenario_nested_source_in_function_body_still_forms_an_edge() {
    let scenario = corpus::scenario("nested_source_in_function_body");
    let world = start(&scenario);

    assert_eq!(world.source_targets(FileId(0)), ["w/b.R"]);
}

/// Statement order does not affect edge recognition.
#[test]
fn test_scenario_source_after_bindings_still_forms_an_edge() {
    let scenario = corpus::scenario("source_after_bindings");
    let world = start(&scenario);

    assert_eq!(world.source_targets(FileId(0)), ["w/b.R"]);
}

/// A nested `library()` is a dependency, not a load-time attachment.
#[test]
fn test_scenario_library_in_function_body_counts_only_as_a_dependency() {
    let scenario = corpus::scenario("library_in_function_body");
    let world = start(&scenario);

    assert_eq!(world.attached_packages(FileId(0)), NO_PACKAGES);
    assert_eq!(world.attached_packages_anywhere(FileId(0)), ["pkga"]);
}

/// `sourceDir(".")` resolves sibling scripts but excludes the sourcing file.
#[test]
fn test_scenario_shallow_source_dir_resolves_sibling_scripts() {
    let scenario = corpus::scenario("shallow_source_dir");
    let world = start(&scenario);

    assert_eq!(world.source_targets(FileId(0)), ["w/b.R"]);
}

/// `targets::tar_source("R")` resolves package scripts from the package root.
#[test]
fn test_scenario_file_or_dir_source_resolves_a_file_target() {
    let scenario = corpus::scenario("file_or_dir_source_at_a_file");
    let world = start(&scenario);

    assert_eq!(world.source_targets(FileId(0)), ["p/mypkg/R/b.R"]);
}

/// `sourceDir()` walks one level, so it excludes the nested script.
#[test]
fn test_scenario_shallow_source_dir_excludes_nested() {
    let scenario = corpus::scenario("shallow_source_dir_excludes_nested");
    let world = start(&scenario);

    assert_eq!(world.source_targets(FileId(0)), ["w/b.R"]);
}

/// Unlike `sourceDir()`, `tar_source()` includes the nested script.
#[test]
fn test_scenario_recursive_source_dir_includes_nested() {
    let scenario = corpus::scenario("recursive_source_dir_includes_nested");
    let world = start(&scenario);

    assert_eq!(world.source_targets(FileId(0)), ["w/b.R", "w/sub/c.R"]);
}

/// `local()` runs eagerly, so its nested `source()` call forms an edge.
#[test]
fn test_scenario_source_in_eager_block_forms_an_edge() {
    let scenario = corpus::scenario("source_in_eager_block");
    let world = start(&scenario);

    assert_eq!(world.source_targets(FileId(0)), ["w/b.R"]);
}

/// `quote()` does not evaluate its argument, so its nested `source()` call
/// forms no edge.
#[test]
fn test_scenario_quote_suppresses_source_effect() {
    let scenario = corpus::scenario("quote_suppresses_source_effect");
    let world = start(&scenario);

    assert_eq!(world.source_targets(FileId(0)), NO_TARGETS);
}

/// A `bquote()` hole evaluates its contents, so its nested `source()` call
/// forms an edge.
#[test]
fn test_scenario_quote_hole_escapes_source_effect() {
    let scenario = corpus::scenario("quote_hole_escapes_source_effect");
    let world = start(&scenario);

    assert_eq!(world.source_targets(FileId(0)), ["w/b.R"]);
}

/// A later binding cannot suppress an earlier call, unlike
/// [`test_scenario_same_file_shadow_suppresses_the_edge()`], where the binding
/// comes first.
#[test]
fn test_scenario_shadow_after_source_call_keeps_the_edge() {
    let scenario = corpus::scenario("shadow_after_source_call");
    let world = start(&scenario);

    assert_eq!(world.source_targets(FileId(0)), ["w/b.R"]);
}

#[test]
fn test_scenario_recursive_source_dir_resolves_package_scripts() {
    let scenario = corpus::scenario("recursive_source_dir_in_package");
    let world = start(&scenario);

    assert_eq!(world.source_targets(FileId(0)), ["p/mypkg/R/b.R"]);
}

// == Package re-export cycles ==

/// Checks the resolved definition and the metadata retained by NAMESPACE parsing.
#[test]
fn test_scenario_acyclic_reexport_chain_resolves_to_the_definition() {
    let scenario = corpus::scenario("acyclic_reexport_chain_resolves_to_the_definition");
    let world = start(&scenario);
    let fired = recovery::fired();

    assert_eq!(
        world.package_resolve(PackageId(0), "exp_a", NamespaceVisibility::Exported),
        ["p/pkgb/R/a.R"]
    );
    assert_eq!(world.package_exports(PackageId(0)), ["exp_a"]);
    assert_eq!(world.package_imported_from(PackageId(0)), [(
        "exp_a".to_string(),
        "pkgb".to_string()
    )]);
    assert!(fired.is_empty());
}

/// Namespace overrides and explicit file ownership can hide incorrect metadata
/// paths from resolution tests, so check the package layout directly.
#[test]
fn test_scenario_package_description_sits_at_the_package_root() {
    let scenario = corpus::scenario("acyclic_reexport_chain_resolves_to_the_definition");
    let world = start(&scenario);

    assert_eq!(
        world.package_description_path(PackageId(1)),
        "p/pkgb/DESCRIPTION"
    );
    assert_eq!(
        world.package_resolve(PackageId(1), "exp_a", NamespaceVisibility::Exported),
        ["p/pkgb/R/a.R"]
    );
}

#[test]
fn test_scenario_library_package_description_sits_in_the_library_root() {
    let scenario = corpus::scenario("mutual_reexport_has_no_terminal_definition");
    let world = start(&scenario);

    assert_eq!(
        world.package_description_path(PackageId(0)),
        "libs/lib0/DESCRIPTION"
    );
}

/// Both package queries receive their own fallback. Recovery firings therefore
/// do not identify which package was Salsa's repeated key.
#[test]
fn test_scenario_mutual_reexport_has_no_terminal_definition() {
    let scenario = corpus::scenario("mutual_reexport_has_no_terminal_definition");
    let world = start(&scenario);
    let mut fired = recovery::fired();
    fired.sort();
    fired.dedup();

    assert_eq!(
        world.package_resolve(PackageId(0), "exp_a", NamespaceVisibility::Exported),
        NO_TARGETS
    );
    assert_eq!(fired, [
        "Package::resolve(lib0, exp_a, Exported)",
        "Package::resolve(lib1, exp_a, Exported)"
    ]);
}

#[test]
fn test_scenario_reexport_chain_terminates_at_a_local_export() {
    let scenario = corpus::scenario("reexport_chain_terminates_at_a_local_export");
    let world = start(&scenario);
    let fired = recovery::fired();

    assert_eq!(
        world.package_resolve(PackageId(0), "exp_a", NamespaceVisibility::Exported),
        ["p/pkgw/R/a.R"]
    );
    assert!(fired.is_empty());
}

/// Resolving both names before and after each edit verifies that changing `pkgw`'s terminal definition invalidates cached re-export results.
#[test]
fn test_scenario_rename_moves_the_end_of_a_reexport_chain() {
    let scenario = corpus::scenario("rename_moves_the_end_of_a_reexport_chain");
    let mut world = start(&scenario);

    assert_eq!(
        world.package_resolve(PackageId(0), "exp_a", NamespaceVisibility::Exported),
        ["p/pkgw/R/a.R"]
    );
    assert_eq!(
        world.package_resolve(PackageId(0), "exp_b", NamespaceVisibility::Exported),
        NO_TARGETS
    );

    world.apply(&scenario.ops[1], &mut Unobserved);

    assert_eq!(
        world.package_resolve(PackageId(0), "exp_a", NamespaceVisibility::Exported),
        NO_TARGETS
    );
    assert_eq!(
        world.package_resolve(PackageId(0), "exp_b", NamespaceVisibility::Exported),
        ["p/pkgw/R/a.R"]
    );

    world.apply(&scenario.ops[4], &mut Unobserved);

    assert_eq!(
        world.package_resolve(PackageId(0), "exp_a", NamespaceVisibility::Exported),
        ["p/pkgw/R/a.R"]
    );
    assert_eq!(
        world.package_resolve(PackageId(0), "exp_b", NamespaceVisibility::Exported),
        NO_TARGETS
    );
    assert!(recovery::fired().is_empty());
}

/// The attach reaches package resolution through `ImportLayer::Package`.
#[test]
fn test_scenario_attached_package_consumer_resolves_a_reexport() {
    let scenario = corpus::scenario("attached_package_consumer_resolves_a_reexport");
    let world = start(&scenario);
    let fired = recovery::fired();

    assert_eq!(world.file_resolve(FileId(1), "exp_a"), ["p/pkgw/R/a.R"]);
    assert!(fired.is_empty());
}

/// Consumer lookup must reach recovery as well as the direct package entry.
#[test]
fn test_scenario_attached_package_consumer_degrades_on_a_reexport_cycle() {
    let scenario = corpus::scenario("attached_package_consumer_degrades_on_a_reexport_cycle");
    let world = start(&scenario);
    let mut fired = recovery::fired();
    fired.sort();
    fired.dedup();

    assert_eq!(world.file_resolve(FileId(0), "exp_a"), NO_TARGETS);
    assert_eq!(fired, [
        "Package::resolve(lib0, exp_a, Exported)",
        "Package::resolve(lib1, exp_a, Exported)"
    ]);
}

/// A package's own `importFrom` reaches a consumer file through
/// `ImportLayer::From`, the collation-wide re-export layer, rather than
/// through an attach.
#[test]
fn test_scenario_namespace_import_layer_consumer_resolves_a_reexport() {
    let scenario = corpus::scenario("namespace_import_layer_consumer_resolves_a_reexport");
    let world = start(&scenario);
    let fired = recovery::fired();

    assert_eq!(world.file_resolve(FileId(0), "exp_a"), ["p/pkgd/R/a.R"]);
    assert!(fired.is_empty());
}

/// An attached package exporting `source` shadows the consumer's bare
/// `source()` effect through `package_binding()`, the same way a local shadow
/// suppresses the edge in [`test_scenario_same_file_shadow_suppresses_the_edge()`].
#[test]
fn test_scenario_package_export_shadows_the_source_effect() {
    let scenario = corpus::scenario("package_export_shadows_the_source_effect");
    let world = start(&scenario);

    assert_eq!(world.source_targets(FileId(0)), NO_TARGETS);
}
