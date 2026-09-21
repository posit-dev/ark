//! Fuzz suite entry points. See [`crate::fuzz`] for the model and coverage, and
//! `crates/oak_db/fuzz/README.md` for commands, CI policy, and artifacts.
//!
//! Pull request CI runs these mutation and replay checks and type-checks the
//! adapter. Run `just fuzz-explore` locally to exercise the libFuzzer integration.
//!
//! `just fuzz` runs every block. `just fuzz-replay-seed SEED` reproduces one
//! with operation tracing, which changes timing. The seed generates the starting
//! corpus, and [`walk_seed()`] derives a seed for each mutation session.
//!
//! Before each operation, the harness writes the scenario to a per-process
//! artifact under `target/oak_fuzz/`. Inspect it after a hang or abort.

mod scenarios;

use std::collections::HashMap;
use std::collections::HashSet;

use mutatis::check::Check;
use mutatis::check::CheckError;
use mutatis::check::CheckResult;
use mutatis::Session;

use crate::fuzz::corpus;
use crate::fuzz::seed_corpus;
use crate::fuzz::FileId;
use crate::fuzz::Runner;
use crate::fuzz::Scenario;
use crate::fuzz::ScenarioMutator;
use crate::fuzz::WorkspaceSpec;
use crate::fuzz::World;
use crate::recovery;

/// Limit the default suite to a quick regression check.
const SMOKE_ITERS: usize = 50;

/// Keep each parallel block below CI's 60-second timeout.
const BLOCK_ITERS: usize = 3000;

/// Cap shrinking because every attempt reruns the full scenario.
const SHRINK_ITERS: usize = 150;

/// Run each block as several walks from its unmutated generated corpus.
///
/// `Check` mutates its corpus in place. `remove_file()` removes a program and
/// its source edges, while `add_file()` adds one binding, so a long walk drifts
/// toward small workspaces without cycles. Restarting each walk continues to
/// exercise the generated cyclic source motifs.
const WALKS: usize = 6;

fn check_block(block: u64, iters: usize) {
    let runner = Runner::open();

    require_package_resolve_recovery(&runner, &seed_corpus(block));

    for (walk, iters) in walk_iters(iters).enumerate() {
        let result = Check::new()
            .iters(iters)
            .shrink_iters(SHRINK_ITERS)
            .seed(walk_seed(block, walk))
            .run_with(ScenarioMutator, seed_corpus(block), |scenario| {
                runner.check(scenario)
            });
        report(result, &runner);
    }
}

/// Splits a block's budget across [`WALKS`] runs, allocating the remainder to
/// earlier walks.
fn walk_iters(iters: usize) -> impl Iterator<Item = usize> {
    (0..WALKS).map(move |walk| iters / WALKS + usize::from(walk < iters % WALKS))
}

/// Derives each walk's mutation seed. Every walk starts from
/// `seed_corpus(block)`, not another walk's mutated corpus.
fn walk_seed(block: u64, walk: usize) -> u64 {
    block * WALKS as u64 + walk as u64
}

/// Verifies that every block's initial corpus reaches `Package::resolve()`
/// cycle recovery. Fuzz blocks only fail on panics and hangs, and mutations can
/// create unrelated re-export cycles that would mask a seed corpus that no
/// longer exercises recovery.
fn require_package_resolve_recovery(runner: &Runner, corpus: &[Scenario]) {
    let reached = corpus
        .iter()
        .any(|scenario| package_resolve_recovered(runner, scenario));

    assert!(reached);
}

/// Ensures every seed block reaches testthat's loader. The load context verifies
/// generated paths and package ownership, and remains observable through source
/// cycles that can prevent definition resolution.
#[test]
fn test_every_block_seed_corpus_reaches_the_testthat_loader() {
    for block in 0..6 {
        let corpus = seed_corpus(block);
        assert!(corpus.iter().any(testthat_loads_a_test_file));
    }
}

fn testthat_loads_a_test_file(scenario: &Scenario) -> bool {
    let Some(test) = scenario
        .initial
        .files
        .iter()
        .position(|file| file.path.starts_with("tests/testthat/test-"))
    else {
        return false;
    };

    let world = World::materialize(&scenario.initial);
    let layers = world.import_layers(FileId(test));
    layers.iter().any(|layer| layer == "Package(testthat)") &&
        layers
            .iter()
            .any(|layer| layer.contains("/tests/testthat/helper-"))
}

/// Scan beyond the six canonical blocks because independent rotation does not
/// guarantee this shape appears in each block.
#[test]
fn test_setup_interleaved_shape_appears() {
    let found = (0u64..50).flat_map(seed_corpus).any(|scenario| {
        scenario
            .initial
            .files
            .iter()
            .any(|file| file.path.contains("/setup-"))
    });
    assert!(found, "no seed in 0..50 produced a SetupInterleaved shape");
}

#[test]
fn test_teardown_excluded_shape_appears() {
    let found = (0u64..50).flat_map(seed_corpus).any(|scenario| {
        scenario
            .initial
            .files
            .iter()
            .any(|file| file.path.contains("/teardown-"))
    });
    assert!(found, "no seed in 0..50 produced a TeardownExcluded shape");
}

#[test]
fn test_nested_non_testthat_file_shape_appears() {
    let found = (0u64..50).flat_map(seed_corpus).any(|scenario| {
        scenario
            .initial
            .files
            .iter()
            .any(|file| file.path.contains("tests/testthat/sub/"))
    });
    assert!(
        found,
        "no seed in 0..50 produced a NestedNonTestthatFile shape"
    );
}

/// `loader_name()` observes loader coverage even when source cycles prevent
/// definition resolution. Each block needs only one rotating Shiny entry.
#[test]
fn test_every_block_seed_corpus_reaches_the_shiny_loader() {
    for block in 0..6 {
        let corpus = seed_corpus(block);
        assert!(corpus.iter().any(some_file_reaches_the_shiny_loader));
    }
}

fn some_file_reaches_the_shiny_loader(scenario: &Scenario) -> bool {
    let Some(entry) = scenario
        .initial
        .files
        .iter()
        .position(|file| is_shiny_entry_basename(&file.path))
    else {
        return false;
    };

    let world = World::materialize(&scenario.initial);
    loader_is_shiny(&world, FileId(entry))
}

fn is_shiny_entry_basename(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    matches!(name, "app.R" | "ui.R" | "server.R")
}

/// Scan beyond the six canonical seeds because independent shape rotation
/// does not guarantee each shape appears in every block.
#[test]
fn test_disabled_autoload_shape_drops_its_r_siblings() {
    let found = (0u64..50)
        .flat_map(seed_corpus)
        .any(|scenario| disables_autoload_for_an_r_sibling(&scenario));
    assert!(found, "no seed in 0..50 produced a DisabledAutoload shape");
}

/// `_disable_autoload.R` is presence-driven, leaving `app.R` classified while
/// unclassifying its `R/` siblings.
fn disables_autoload_for_an_r_sibling(scenario: &Scenario) -> bool {
    let has_disable_file = scenario
        .initial
        .files
        .iter()
        .any(|file| file.path.ends_with("_disable_autoload.R"));
    let Some(sibling) = scenario.initial.files.iter().position(|file| {
        file.path.starts_with("R/") && !file.path.ends_with("_disable_autoload.R")
    }) else {
        return false;
    };
    if !has_disable_file {
        return false;
    }

    let world = World::materialize(&scenario.initial);
    world.loader_name(FileId(sibling)).is_none()
}

#[test]
fn test_ui_server_pair_shape_gives_both_files_the_shiny_loader() {
    let found = (0u64..50)
        .flat_map(seed_corpus)
        .any(|scenario| ui_and_server_both_reach_shiny(&scenario));
    assert!(found, "no seed in 0..50 produced a UiServerPair shape");
}

fn ui_and_server_both_reach_shiny(scenario: &Scenario) -> bool {
    let Some(ui) = scenario
        .initial
        .files
        .iter()
        .position(|file| file.path == "ui.R")
    else {
        return false;
    };
    let Some(server) = scenario
        .initial
        .files
        .iter()
        .position(|file| file.path == "server.R")
    else {
        return false;
    };

    let world = World::materialize(&scenario.initial);
    loader_is_shiny(&world, FileId(ui)) && loader_is_shiny(&world, FileId(server))
}

/// Query loader classification directly because random `library(shiny)` and
/// `source()` edges can make [`World::import_layers()`] look equivalent.
fn loader_is_shiny(world: &World, file: FileId) -> bool {
    world.loader_name(file) == Some("This Shiny app")
}

#[test]
fn test_package_inst_app_shape_reaches_the_shiny_loader() {
    let found = (0u64..50)
        .flat_map(seed_corpus)
        .any(|scenario| package_owned_app_reaches_shiny(&scenario));
    assert!(found, "no seed in 0..50 produced a PackageInstApp shape");
}

/// `inst/app/` files bypass package collation, so package classification
/// returns `None` and lets Shiny claim the entry.
fn package_owned_app_reaches_shiny(scenario: &Scenario) -> bool {
    let Some(entry) = scenario
        .initial
        .files
        .iter()
        .position(|file| file.path == "inst/app/app.R")
    else {
        return false;
    };

    let world = World::materialize(&scenario.initial);
    loader_is_shiny(&world, FileId(entry))
}

#[test]
fn test_nested_app_file_shape_appears() {
    let found = (0u64..50)
        .flat_map(seed_corpus)
        .any(|scenario| has_nested_app_file(&scenario));
    assert!(found, "no seed in 0..50 produced a NestedAppFile shape");
}

/// `loader_name()` cannot distinguish joining the outer app from incorrectly
/// rooting a new one because both report the same loader. The deterministic
/// scenario verifies the resolution result.
fn has_nested_app_file(scenario: &Scenario) -> bool {
    scenario
        .initial
        .files
        .iter()
        .any(|file| file.path == "app.R") &&
        scenario
            .initial
            .files
            .iter()
            .any(|file| file.path == "R/app.R")
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
#[ignore = "opt-in: just fuzz-replay-seed <seed>"]
fn test_replay_block() {
    let seed = match std::env::var("OAK_FUZZ_SEED") {
        Ok(seed) => match seed.parse::<u64>() {
            Ok(seed) => seed,
            Err(err) => panic!("OAK_FUZZ_SEED is not a u64: {err}"),
        },
        Err(_) => panic!("set OAK_FUZZ_SEED, or run `just fuzz-replay-seed <seed>`"),
    };
    check_block(seed, BLOCK_ITERS);
}

/// Reproduce a saved scenario, whether written by `cargo fuzz` or by hand.
/// Replay needs no fuzzing toolchain, so a crash the driver found is
/// reproducible from a checkout with the stable toolchain.
#[test]
#[ignore = "opt-in: just fuzz-replay-scenario <path>"]
fn test_replay_scenario() {
    let path = match std::env::var("OAK_FUZZ_SCENARIO") {
        Ok(path) => path,
        Err(_) => panic!("set OAK_FUZZ_SCENARIO, or run `just fuzz-replay-scenario <path>`"),
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

    for case in corpus::cases() {
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
