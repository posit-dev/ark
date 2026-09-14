//! Exercises Salsa queries across mutated workspaces and edit histories.
//!
//! Each selected query must complete without panicking or hanging. Recovery
//! firings provide context but do not identify Salsa's repeated key.
//!
//! [`generate::seed_corpus()`] supplies fixed starting scenarios, and
//! [`mutate::ScenarioMutator`] derives the rest. Failures replay the shrunken
//! scenario so its trace, panic location, and artifact agree. Save the printed
//! scenario as an explicit [`Scenario`] test.
//!
//! ```text
//! just fuzz
//! just fuzz-seed 1234
//! ```
//!
//! `SEED` controls both the starting corpus and the mutation session. Rerun
//! `just fuzz-seed SEED` to reproduce a block.
//!
//! Before each operation, the harness writes the scenario to a per-process
//! artifact under `target/oak_fuzz/`. Inspect it after a hang or abort.
//! `just fuzz-seed` also traces every operation, which changes timing.
//!
//! # Coverage
//!
//! [`Query`] covers 18 of the 50 tracked queries in the Salsa inventory. It
//! includes the production roots `diagnostics()`, `imports()`, `imports_at()`,
//! `resolve_at()`, `resolve()`, `used_packages()`, and `sourced_by()`, all five
//! workspace aggregates, and cold entry into the six file-keyed queries with
//! `cycle_result` handlers.
//!
//! Mutation reaches every `EffectRecipe` variant, both invocation forms, and
//! all three `SourceProvider` walks, including an effect escaped through a
//! `bquote()` hole.
//!
//! `Package::resolve()` is excluded because these workspaces have no NAMESPACE
//! re-exports. Testthat and shiny layouts, file renaming, and metadata or
//! revision edits stay out of reach. Queries outside [`Query`] are covered only
//! as dependencies, not as entry points.

mod artifact;
mod build;
mod choose;
mod corpus;
mod generate;
mod mutate;
mod panics;
mod run;
mod scenario;
mod spec;

use mutatis::check::Check;
use mutatis::check::CheckError;
use mutatis::check::CheckResult;
use mutatis::Session;

use crate::recovery;
use crate::tests::fuzz::artifact::Artifact;
use crate::tests::fuzz::generate::seed_corpus;
use crate::tests::fuzz::mutate::ScenarioMutator;
use crate::tests::fuzz::run::run;
use crate::tests::fuzz::run::run_scenario;
use crate::tests::fuzz::run::start;
use crate::tests::fuzz::run::World;
use crate::tests::fuzz::scenario::Scenario;
use crate::tests::fuzz::spec::FileId;

/// Limit the default suite to a quick regression check.
const SMOKE_ITERS: usize = 50;

/// Keep each parallel block below CI's 60-second timeout.
const BLOCK_ITERS: usize = 3000;

/// Cap shrinking because every attempt reruns the full scenario.
const SHRINK_ITERS: usize = 150;

fn check_block(block: u64, iters: usize) {
    let artifact = Artifact::open();
    let _hook = panics::install();
    let result = Check::new()
        .iters(iters)
        .shrink_iters(SHRINK_ITERS)
        .seed(block)
        .run_with(ScenarioMutator, seed_corpus(block), |scenario| {
            run_scenario(scenario, &artifact)
        });
    report(result, &artifact);
}

/// Replay the shrunken failure because the last candidate evaluated during
/// shrinking may have passed and overwritten the artifact.
fn report(result: CheckResult<Scenario>, artifact: &Artifact) {
    let Err(error) = result else {
        artifact.clear();
        return;
    };
    let failure = match error {
        CheckError::Failed(failure) => failure,
        other => panic!("fuzz check did not run: {other}"),
    };

    eprintln!("{}", failure.message);
    eprintln!("artifact: {}", artifact.path().display());
    run(&failure.value, artifact, true);

    // A concrete `Scenario` makes `run()` deterministic, so returning means
    // the failure did not reproduce.
    panic!("fuzz failure did not reproduce on replay");
}

#[test]
fn test_fuzz_smoke() {
    check_block(0, SMOKE_ITERS);
}

// == Opt-in suite ==

#[test]
#[ignore = "opt-in: just fuzz"]
fn test_block_0() {
    check_block(0, BLOCK_ITERS);
}

#[test]
#[ignore = "opt-in: just fuzz"]
fn test_block_1() {
    check_block(1, BLOCK_ITERS);
}

#[test]
#[ignore = "opt-in: just fuzz"]
fn test_block_2() {
    check_block(2, BLOCK_ITERS);
}

#[test]
#[ignore = "opt-in: just fuzz"]
fn test_block_3() {
    check_block(3, BLOCK_ITERS);
}

#[test]
#[ignore = "opt-in: just fuzz"]
fn test_block_4() {
    check_block(4, BLOCK_ITERS);
}

#[test]
#[ignore = "opt-in: just fuzz"]
fn test_block_5() {
    check_block(5, BLOCK_ITERS);
}

#[test]
#[ignore = "opt-in: just fuzz-seed <seed>"]
fn test_replay_block() {
    let seed = match std::env::var("OAK_FUZZ_SEED") {
        Ok(seed) => match seed.parse::<u64>() {
            Ok(seed) => seed,
            Err(err) => panic!("OAK_FUZZ_SEED is not a u64: {err}"),
        },
        Err(_) => panic!("set OAK_FUZZ_SEED, or run `just fuzz-seed <seed>`"),
    };
    check_block(seed, BLOCK_ITERS);
}

// == Mutation coverage ==

/// Credit only increases over the seed baseline, which already contains cycle
/// transitions. Each measurement uses a separate database so diagnostics do not
/// change the no-panic runs' cold-entry order.
#[test]
fn test_mutated_histories_close_and_reopen_recognized_cycles() {
    const MUTATIONS: usize = 400;

    let mut session = Session::new().seed(0);
    let mut corpus = seed_corpus(0);
    let baseline: Vec<Transitions> = corpus.iter().map(cycle_transitions).collect();

    let mut closed = false;
    let mut reopened = false;

    for round in 0..MUTATIONS {
        let entry = round % corpus.len();
        if session
            .mutate_with(&mut ScenarioMutator, &mut corpus[entry])
            .is_err()
        {
            continue;
        }

        let after = cycle_transitions(&corpus[entry]);
        closed |= after.closed > baseline[entry].closed;
        reopened |= after.reopened > baseline[entry].reopened;
    }

    assert!(closed);
    assert!(reopened);
}

struct Transitions {
    closed: usize,
    reopened: usize,
}

fn cycle_transitions(scenario: &Scenario) -> Transitions {
    let mut transitions = Transitions {
        closed: 0,
        reopened: 0,
    };
    let mut world = World::materialize(&scenario.initial);
    let mut cyclic = world.any_source_cycle();

    for op in &scenario.ops {
        world.apply(op);
        let now = world.any_source_cycle();
        transitions.closed += usize::from(now && !cyclic);
        transitions.reopened += usize::from(cyclic && !now);
        cyclic = now;
    }

    transitions
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
