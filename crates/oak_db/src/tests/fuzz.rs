//! Exercises Salsa queries across generated workspaces and edit histories.
//!
//! Each selected query must complete without panicking or hanging. Recovery
//! firings add failure context but cannot identify Salsa's repeated key.
//!
//! A seed reproduces a scenario only for an unchanged generator and RNG. Save
//! failures as explicit [`Scenario`] tests.
//!
//! ```text
//! just fuzz
//! just fuzz-seed 1234
//! ```
//!
//! `just fuzz-seed` reports each operation eagerly because hangs and aborts do
//! not reach the unwind report. Set `OAK_FUZZ_TRACE=1` on other runs when
//! investigating those failures.
//!
//! # Coverage
//!
//! [`Query`] covers 18 of the 50 tracked queries in the Salsa inventory. It
//! includes the production roots `diagnostics()`, `imports()`, `imports_at()`,
//! `resolve_at()`, `resolve()`, `used_packages()`, and `sourced_by()`, all five
//! workspace aggregates, and cold entry into the six file-keyed queries with
//! `cycle_result` handlers.
//!
//! `Package::resolve()` is excluded because the generated workspaces have no
//! NAMESPACE re-exports. Directory sourcing is covered only by explicit corpus
//! scenarios. The generator also excludes testthat and shiny layouts, file
//! creation, removal, renaming, and metadata or revision edits. Queries outside
//! [`Query`] are covered only as dependencies, not as entry points.

mod build;
mod corpus;
mod generate;
mod run;
mod scenario;
mod spec;

use std::ops::Range;

use crate::recovery;
use crate::tests::fuzz::generate::generate;
use crate::tests::fuzz::run::run;
use crate::tests::fuzz::run::start;
use crate::tests::fuzz::run::World;
use crate::tests::fuzz::spec::FileId;

/// Keep the default suite small enough to catch basic regressions without
/// materially increasing test time.
const SMOKE: Range<u64> = 0..8;

/// Keep each block below the CI profile's 60-second termination threshold.
/// Separate tests let nextest run the blocks in parallel.
const SEEDS_PER_TEST: u64 = 600;

fn run_seeds(seeds: Range<u64>) {
    for seed in seeds {
        run_seed(seed);
    }
}

fn run_seed(seed: u64) {
    for scenario in generate(seed) {
        run(&scenario);
    }
}

fn seed_block(block: u64) -> Range<u64> {
    let start = block * SEEDS_PER_TEST;
    start..start + SEEDS_PER_TEST
}

#[test]
fn test_fuzz_smoke() {
    run_seeds(SMOKE);
}

// == Opt-in suite ==

#[test]
#[ignore = "opt-in: just fuzz"]
fn test_seeds_0() {
    run_seeds(seed_block(0));
}

#[test]
#[ignore = "opt-in: just fuzz"]
fn test_seeds_1() {
    run_seeds(seed_block(1));
}

#[test]
#[ignore = "opt-in: just fuzz"]
fn test_seeds_2() {
    run_seeds(seed_block(2));
}

#[test]
#[ignore = "opt-in: just fuzz"]
fn test_seeds_3() {
    run_seeds(seed_block(3));
}

#[test]
#[ignore = "opt-in: just fuzz"]
fn test_seeds_4() {
    run_seeds(seed_block(4));
}

#[test]
#[ignore = "opt-in: just fuzz"]
fn test_seeds_5() {
    run_seeds(seed_block(5));
}

#[test]
#[ignore = "opt-in: just fuzz-seed <seed>"]
fn test_replay_seed() {
    let seed = match std::env::var("OAK_FUZZ_SEED") {
        Ok(seed) => match seed.parse::<u64>() {
            Ok(seed) => seed,
            Err(err) => panic!("OAK_FUZZ_SEED is not a u64: {err}"),
        },
        Err(_) => panic!("set OAK_FUZZ_SEED, or run `just fuzz-seed <seed>`"),
    };
    run_seed(seed);
}

// == Generator coverage ==

/// Generated edits must create and remove recognized cycles.
///
/// Use a separate database so diagnostics do not change the no-panic runs'
/// cold-entry order.
#[test]
fn test_generated_histories_close_and_reopen_recognized_cycles() {
    let mut closed = 0;
    let mut reopened = 0;

    for seed in SMOKE {
        for scenario in generate(seed) {
            let mut world = World::materialize(&scenario.initial);
            let mut cyclic = world.any_source_cycle();
            for op in &scenario.ops {
                world.apply(op);
                let now = world.any_source_cycle();
                closed += usize::from(now && !cyclic);
                reopened += usize::from(cyclic && !now);
                cyclic = now;
            }
        }
    }

    assert!(closed > 0);
    assert!(reopened > 0);
}

// == Scenario invariants ==

const NO_TARGETS: [&str; 0] = [];
const NO_PACKAGES: [&str; 0] = [];

#[test]
fn test_scenario_acyclic_pair_closes_then_reopens() {
    let scenario = corpus::case("acyclic_pair_closes_then_reopens");
    let mut world = start(&scenario);

    assert_eq!(world.source_targets(FileId(0)), ["w/b.R"]);
    assert_eq!(world.source_targets(FileId(1)), NO_TARGETS);
    assert!(!world.any_source_cycle());

    world.apply(&scenario.ops[0]);

    assert!(world.source_cycle_reported(FileId(0)));
    assert!(world.source_cycle_reported(FileId(1)));
    // `NoopImportsResolver` resolves no `source()` callee, so recovery removes
    // the cycle-forming edges from the reporting index.
    assert_eq!(world.source_targets(FileId(0)), NO_TARGETS);

    world.apply(&scenario.ops[1]);

    assert!(!world.any_source_cycle());
    assert_eq!(world.source_targets(FileId(0)), ["w/b.R"]);
}

#[test]
fn test_scenario_mutual_pair_opens_then_closes_again() {
    let scenario = corpus::case("mutual_pair_opens_then_closes_again");
    let mut world = start(&scenario);

    assert!(world.source_cycle_reported(FileId(0)));
    assert!(world.source_cycle_reported(FileId(1)));

    world.apply(&scenario.ops[0]);

    assert!(!world.any_source_cycle());
    assert_eq!(world.source_targets(FileId(0)), ["w/b.R"]);

    world.apply(&scenario.ops[1]);

    assert!(world.source_cycle_reported(FileId(0)));
    assert!(world.source_cycle_reported(FileId(1)));
}

/// Require `cross_file_layers()` to become Salsa's repeated key when entered
/// cold through a package with an `R/` collation.
#[test]
fn test_scenario_package_cold_entry_reaches_cross_file_layers_recovery() {
    let scenario = corpus::case("package_cold_entry_reaches_cross_file_layers_recovery");
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

/// Verify that a locally shadowed `source()` contributes no source edge.
#[test]
fn test_scenario_same_file_shadow_suppresses_the_edge() {
    let scenario = corpus::case("same_file_shadow_suppresses_the_edge");
    let world = start(&scenario);

    assert_eq!(world.source_targets(FileId(0)), NO_TARGETS);
}

/// A nested `source()` forms an edge even when the function is never called.
#[test]
fn test_scenario_nested_source_in_function_body_still_forms_an_edge() {
    let scenario = corpus::case("nested_source_in_function_body");
    let world = start(&scenario);

    assert_eq!(world.source_targets(FileId(0)), ["w/b.R"]);
}

/// Statement order does not affect edge recognition.
#[test]
fn test_scenario_source_after_bindings_still_forms_an_edge() {
    let scenario = corpus::case("source_after_bindings");
    let world = start(&scenario);

    assert_eq!(world.source_targets(FileId(0)), ["w/b.R"]);
}

/// A nested `library()` is a dependency, not a load-time attachment.
#[test]
fn test_scenario_library_in_function_body_counts_only_as_a_dependency() {
    let scenario = corpus::case("library_in_function_body");
    let world = start(&scenario);

    assert_eq!(world.attached_packages(FileId(0)), NO_PACKAGES);
    assert_eq!(world.attached_packages_anywhere(FileId(0)), ["pkga"]);
}

/// `sourceDir(".")` resolves sibling scripts but excludes the sourcing file.
#[test]
fn test_scenario_shallow_source_dir_resolves_sibling_scripts() {
    let scenario = corpus::case("shallow_source_dir");
    let world = start(&scenario);

    assert_eq!(world.source_targets(FileId(0)), ["w/b.R"]);
}

/// `targets::tar_source("R")` resolves package scripts from the package root.
#[test]
fn test_scenario_file_or_dir_source_resolves_a_file_target() {
    let scenario = corpus::case("file_or_dir_source_at_a_file");
    let world = start(&scenario);

    assert_eq!(world.source_targets(FileId(0)), ["p/mypkg/R/b.R"]);
}

#[test]
fn test_scenario_recursive_source_dir_resolves_package_scripts() {
    let scenario = corpus::case("recursive_source_dir_in_package");
    let world = start(&scenario);

    assert_eq!(world.source_targets(FileId(0)), ["p/mypkg/R/b.R"]);
}
