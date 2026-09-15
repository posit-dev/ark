//! Fuzz suite entry points. See [`crate::fuzz`] for the model and coverage, and
//! `crates/oak_db/fuzz/README.md` for commands, CI policy, and artifacts.
//!
//! Pull request CI runs these mutation and replay checks and type-checks the
//! adapter. Run `just fuzz-driver` locally to exercise the libFuzzer integration.
//!
//! `just fuzz` runs every block. `just fuzz-seed SEED` reproduces one with
//! operation tracing, which changes timing. The seed controls both the starting
//! corpus and the mutation session.
//!
//! Before each operation, the harness writes the scenario to a per-process
//! artifact under `target/oak_fuzz/`. Inspect it after a hang or abort.

use std::collections::HashMap;
use std::collections::HashSet;

use mutatis::check::Check;
use mutatis::check::CheckError;
use mutatis::check::CheckResult;
use mutatis::Session;

use crate::fuzz::corpus;
use crate::fuzz::seed_corpus;
use crate::fuzz::start;
use crate::fuzz::FileId;
use crate::fuzz::PackageId;
use crate::fuzz::Runner;
use crate::fuzz::Scenario;
use crate::fuzz::ScenarioMutator;
use crate::fuzz::WorkspaceSpec;
use crate::fuzz::World;
use crate::recovery;
use crate::NamespaceVisibility;

/// Limit the default suite to a quick regression check.
const SMOKE_ITERS: usize = 50;

/// Keep each parallel block below CI's 60-second timeout.
const BLOCK_ITERS: usize = 3000;

/// Cap shrinking because every attempt reruns the full scenario.
const SHRINK_ITERS: usize = 150;

/// Number of `test_block_*` seeds in the opt-in suite.
const BLOCKS: u64 = 6;

fn check_block(block: u64, iters: usize) {
    let runner = Runner::open();
    let result = Check::new()
        .iters(iters)
        .shrink_iters(SHRINK_ITERS)
        .seed(block)
        .run_with(ScenarioMutator, seed_corpus(block), |scenario| {
            runner.check(scenario)
        });
    report(result, &runner);
}

/// Replay the shrunken failure because the last candidate evaluated during
/// shrinking may have passed and overwritten the artifact.
fn report(result: CheckResult<Scenario>, runner: &Runner) {
    let Err(error) = result else {
        runner.clear();
        return;
    };
    let failure = match error {
        CheckError::Failed(failure) => failure,
        other => panic!("fuzz check did not run: {other}"),
    };

    eprintln!("{}", failure.message);
    eprintln!("artifact: {}", runner.artifact_path().display());
    runner.replay(&failure.value);

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

/// Reproduce a saved scenario, whether written by `cargo fuzz` or by hand.
/// Replay needs no fuzzing toolchain, so a crash the driver found is
/// reproducible from a checkout with the stable toolchain.
#[test]
#[ignore = "opt-in: just fuzz-replay <path>"]
fn test_replay_scenario() {
    let path = match std::env::var("OAK_FUZZ_SCENARIO") {
        Ok(path) => path,
        Err(_) => panic!("set OAK_FUZZ_SCENARIO, or run `just fuzz-replay <path>`"),
    };
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) => panic!("cannot read {path}: {err}"),
    };
    let scenario = match Scenario::from_json(&bytes) {
        Ok(scenario) => scenario,
        Err(err) => panic!("{path} is not a scenario: {err:?}"),
    };

    Runner::open().replay(&scenario);
}

/// Seed `cargo fuzz` with scenarios that already satisfy the JSON format.
#[test]
#[ignore = "opt-in: just fuzz-corpus"]
fn test_write_seed_corpus() {
    let dir = match std::env::var("OAK_FUZZ_CORPUS") {
        Ok(dir) => dir,
        Err(_) => panic!("set OAK_FUZZ_CORPUS, or run `just fuzz-corpus`"),
    };
    let dir = std::path::Path::new(&dir);
    std::fs::create_dir_all(dir).unwrap();

    for case in corpus::corpus() {
        write_scenario_json(dir, case.name, &case.scenario);
    }
    for scenario in seed_corpus(0) {
        let name = format!("seed{}_variant{}", scenario.seed, scenario.variant);
        write_scenario_json(dir, &name, &scenario);
    }
}

fn write_scenario_json(dir: &std::path::Path, name: &str, scenario: &Scenario) {
    let json = scenario.to_json().unwrap();
    std::fs::write(dir.join(format!("{name}.json")), json).unwrap();
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

/// Checks graph changes between successive mutations, since scenario operations
/// do not edit package metadata. This ignores imported names and export gates,
/// so a graph cycle need not produce a `Package::resolve()` cycle.
#[test]
fn test_mutated_packages_close_and_reopen_reexport_cycles() {
    const MUTATIONS: usize = 400;

    let mut session = Session::new().seed(0);
    let mut corpus = seed_corpus(0);
    let mut cyclic: Vec<bool> = corpus
        .iter()
        .map(|scenario| has_reexport_cycle(&scenario.initial))
        .collect();

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

        let now = has_reexport_cycle(&corpus[entry].initial);
        closed |= now && !cyclic[entry];
        reopened |= !now && cyclic[entry];
        cyclic[entry] = now;
    }

    assert!(closed);
    assert!(reopened);
}

fn has_reexport_cycle(spec: &WorkspaceSpec) -> bool {
    let edges: HashMap<&str, Vec<&str>> = spec
        .packages
        .iter()
        .map(|package| {
            let sources = package
                .reexports
                .iter()
                .map(|reexport| reexport.from.as_str())
                .collect();
            (package.name.as_str(), sources)
        })
        .collect();

    let mut visiting = HashSet::new();
    let mut done = HashSet::new();
    edges
        .keys()
        .any(|&name| reaches_itself(name, &edges, &mut visiting, &mut done))
}

fn reaches_itself<'a>(
    name: &'a str,
    edges: &HashMap<&'a str, Vec<&'a str>>,
    visiting: &mut HashSet<&'a str>,
    done: &mut HashSet<&'a str>,
) -> bool {
    if done.contains(name) {
        return false;
    }
    if !visiting.insert(name) {
        return true;
    }
    let cyclic = edges.get(name).is_some_and(|sources| {
        sources
            .iter()
            .any(|source| reaches_itself(source, edges, visiting, done))
    });
    visiting.remove(name);
    done.insert(name);
    cyclic
}

/// Require actual recovery from each block's unmutated corpus. A structural
/// cycle alone is insufficient, and mutation must not rescue missing seed coverage.
#[test]
fn test_every_seed_corpus_reaches_the_package_resolve_handler() {
    let runner = Runner::open();
    for block in 0..BLOCKS {
        let reached = seed_corpus(block)
            .iter()
            .any(|scenario| package_resolve_recovered(&runner, scenario));

        assert!(reached, "block {block} never reached `Package::resolve()`");
    }
}

/// The runner resets the recovery log before each scenario, so the firings it
/// leaves behind belong to `scenario` alone.
fn package_resolve_recovered(runner: &Runner, scenario: &Scenario) -> bool {
    if let Err(failure) = runner.check(scenario) {
        panic!("{failure}");
    }
    recovery::fired()
        .iter()
        .any(|entry| entry.starts_with("Package::resolve("))
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

/// A memoized cycle result does not survive a revision bump, so revalidating
/// `cross_file_layers()` after the edit consults its handler again.
#[test]
fn test_scenario_package_edit_revalidates_cross_file_layers_recovery() {
    let scenario = corpus::case("package_edit_revalidates_cross_file_layers_recovery");
    let mut world = start(&scenario);

    // Attribute only the post-edit firings, since the cold entry recovers too.
    recovery::reset();
    for op in &scenario.ops {
        world.apply(op);
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

// == Package re-export cycles ==

/// Checks the resolved definition and the metadata retained by NAMESPACE parsing.
#[test]
fn test_scenario_acyclic_reexport_chain_resolves_to_the_definition() {
    let scenario = corpus::case("acyclic_reexport_chain_resolves_to_the_definition");
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
    let scenario = corpus::case("acyclic_reexport_chain_resolves_to_the_definition");
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
    let scenario = corpus::case("mutual_reexport_has_no_terminal_definition");
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
    let scenario = corpus::case("mutual_reexport_has_no_terminal_definition");
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
    let scenario = corpus::case("reexport_chain_terminates_at_a_local_export");
    let world = start(&scenario);
    let fired = recovery::fired();

    assert_eq!(
        world.package_resolve(PackageId(0), "exp_a", NamespaceVisibility::Exported),
        ["p/pkgw/R/a.R"]
    );
    assert!(fired.is_empty());
}

/// The attach reaches package resolution through `ImportLayer::Package`.
#[test]
fn test_scenario_attached_package_consumer_resolves_a_reexport() {
    let scenario = corpus::case("attached_package_consumer_resolves_a_reexport");
    let world = start(&scenario);
    let fired = recovery::fired();

    assert_eq!(world.file_resolve(FileId(1), "exp_a"), ["p/pkgw/R/a.R"]);
    assert!(fired.is_empty());
}

/// Consumer lookup must reach recovery as well as the direct package entry.
#[test]
fn test_scenario_attached_package_consumer_degrades_on_a_reexport_cycle() {
    let scenario = corpus::case("attached_package_consumer_degrades_on_a_reexport_cycle");
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
    let scenario = corpus::case("namespace_import_layer_consumer_resolves_a_reexport");
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
    let scenario = corpus::case("package_export_shadows_the_source_effect");
    let world = start(&scenario);

    assert_eq!(world.source_targets(FileId(0)), NO_TARGETS);
}
