//! Fuzz suite entry points. See [`crate::fuzz`] for the model and coverage, and
//! `crates/oak_db/fuzz/README.md` for commands, CI policy, and artifacts.
//!
//! Pull request CI runs these mutation and replay checks and type-checks the
//! adapter. Run `just fuzz-explore` locally to exercise the libFuzzer integration.
//!
//! `just fuzz` runs every block. `just fuzz-replay-seed SEED` reproduces one
//! with operation tracing, which changes timing. The seed controls both the
//! starting corpus and the mutation session.
//!
//! Before each operation, the harness writes the scenario to a per-process
//! artifact under `target/oak_fuzz/`. Inspect it after a hang or abort.

mod oracle;
mod scenarios;

use std::collections::HashMap;
use std::collections::HashSet;

use mutatis::check::Check;
use mutatis::check::CheckError;
use mutatis::check::CheckResult;
use mutatis::Session;

use crate::fuzz::corpus;
use crate::fuzz::oracle::campaign;
use crate::fuzz::oracle::compare::Fresh;
use crate::fuzz::seed_corpus;
use crate::fuzz::Runner;
use crate::fuzz::Scenario;
use crate::fuzz::ScenarioMutator;
use crate::fuzz::Unobserved;
use crate::fuzz::WorkspaceSpec;
use crate::fuzz::World;
use crate::recovery;

/// Limit the default suite to a quick regression check.
const SMOKE_ITERS: usize = 50;

/// Keep each parallel block below CI's 60-second timeout.
const BLOCK_ITERS: usize = 3000;

/// Cap shrinking because every attempt reruns the full scenario.
const SHRINK_ITERS: usize = 150;

fn check_block(block: u64, iters: usize) {
    let runner = Runner::open();
    let corpus = seed_corpus(block);

    require_package_resolve_recovery(&runner, &corpus);

    let result = Check::new()
        .iters(iters)
        .shrink_iters(SHRINK_ITERS)
        .seed(block)
        .run_with(ScenarioMutator, corpus, |scenario| runner.check(scenario));
    report(result, &runner);
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
    Runner::open().replay(&saved_scenario("just fuzz-replay-scenario"));
}

/// Replays a saved scenario while comparing resolution against a fresh database. A mismatch is a finding rather than an execution panic, so `test_replay_scenario()` cannot report it. The entry point selects the mode, not the scenario.
#[test]
#[ignore = "opt-in: just fuzz-replay-semantic <path>"]
fn test_replay_semantic_scenario() {
    campaign::replay(&saved_scenario("just fuzz-replay-semantic"), Fresh);
}

/// Reduces a saved scenario that mismatches against a fresh database and
/// reports the reduced mismatch.
#[test]
#[ignore = "opt-in: just fuzz-reduce-semantic <path>"]
fn test_reduce_semantic_scenario() {
    campaign::reduce_and_report(saved_scenario("just fuzz-reduce-semantic"), 0, 1000);
}

/// Sweeps the coverage-guided corpus before minimization can discard
/// coverage-redundant histories with semantic mismatches.
#[test]
#[ignore = "opt-in: just fuzz-sweep-semantic <dir>"]
fn test_sweep_semantic_corpus() {
    let dir = match std::env::var("OAK_FUZZ_CORPUS") {
        Ok(dir) => dir,
        Err(_) => panic!("set OAK_FUZZ_CORPUS, or run `just fuzz-sweep-semantic <dir>`"),
    };
    let summary = campaign::sweep(std::path::Path::new(&dir));
    eprintln!("{}", summary.render());
}

fn saved_scenario(recipe: &str) -> Scenario {
    let path = match std::env::var("OAK_FUZZ_SCENARIO") {
        Ok(path) => path,
        Err(_) => panic!("set OAK_FUZZ_SCENARIO, or run `{recipe} <path>`"),
    };
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) => panic!("cannot read {path}: {err}"),
    };
    match Scenario::from_json(&bytes) {
        Ok(scenario) => scenario,
        Err(err) => panic!("{path} is not a scenario: {err:?}"),
    }
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
        world.apply(op, &mut Unobserved);
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
