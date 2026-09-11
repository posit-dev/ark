//! Exercises Salsa queries across generated workspaces and edit histories.
//!
//! Each selected query must complete without panicking or hanging. Recovery
//! firings provide failure context but cannot identify Salsa's repeated key.
//!
//! A seed reproduces a scenario only while the generator and RNG remain
//! unchanged. Preserve failures as explicit [`Scenario`] tests.
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
//! NAMESPACE re-exports. The generator also excludes `sourceDir()` edges,
//! testthat and shiny layouts, file creation, removal, and renaming, and
//! metadata and revision edits. Queries outside [`Query`] are covered only
//! when reached as dependencies, not as entry points.

mod generate;
mod run;
mod scenario;
mod spec;

use std::ops::Range;

use crate::file_imports::CollationView;
use crate::recovery;
use crate::tests::fuzz::generate::generate;
use crate::tests::fuzz::run::run;
use crate::tests::fuzz::run::start;
use crate::tests::fuzz::run::World;
use crate::tests::fuzz::scenario::Edit;
use crate::tests::fuzz::scenario::Op;
use crate::tests::fuzz::scenario::Query;
use crate::tests::fuzz::scenario::Scenario;
use crate::tests::fuzz::spec::FileId;
use crate::tests::fuzz::spec::FileSpec;
use crate::tests::fuzz::spec::Owner;
use crate::tests::fuzz::spec::WorkspaceSpec;

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

/// Require generated edits to both create and remove recognized cycles.
///
/// Use a separate database because querying diagnostics here would warm Salsa
/// and change the entry orders exercised by the no-panic runs.
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

// == Explicit scenarios ==
//
// These assertions verify that the constructed workspaces expose the source
// edges and cycles required by the no-panic scenarios.

/// Include `base` so `source()` can form edges between the scripts.
fn scripts(files: &[(&str, &str)]) -> WorkspaceSpec {
    WorkspaceSpec {
        installed: vec!["base".to_string()],
        package: None,
        files: file_specs(Owner::Script, files),
    }
}

fn package(name: &str, installed: &[&str], files: &[(&str, &str)]) -> WorkspaceSpec {
    WorkspaceSpec {
        installed: installed.iter().map(|name| name.to_string()).collect(),
        package: Some(name.to_string()),
        files: file_specs(Owner::Package, files),
    }
}

fn file_specs(owner: Owner, files: &[(&str, &str)]) -> Vec<FileSpec> {
    files
        .iter()
        .map(|(path, contents)| FileSpec {
            owner,
            path: path.to_string(),
            contents: contents.to_string(),
        })
        .collect()
}

fn scenario(initial: WorkspaceSpec, cold_entry: Query) -> Scenario {
    Scenario {
        seed: 0,
        variant: 0,
        initial,
        cold_entry,
        ops: vec![],
    }
}

fn replace(file: FileId, contents: &str) -> Op {
    Op::Edit(Edit {
        file,
        contents: contents.to_string(),
    })
}

const NO_TARGETS: [&str; 0] = [];

#[test]
fn test_scenario_acyclic_pair_closes_then_reopens() {
    let initial = scripts(&[
        ("a.R", "source(\"b.R\")\nval_a <- 1\n"),
        ("b.R", "val_b <- 1\n"),
    ]);
    let mut world = start(&scenario(initial, Query::Diagnostics(FileId(0))));

    assert_eq!(world.source_targets(FileId(0)), ["w/b.R"]);
    assert_eq!(world.source_targets(FileId(1)), NO_TARGETS);
    assert!(!world.any_source_cycle());

    world.apply(&replace(FileId(1), "source(\"a.R\")\nval_b <- 1\n"));

    assert!(world.source_cycle_reported(FileId(0)));
    assert!(world.source_cycle_reported(FileId(1)));
    // `NoopImportsResolver` resolves no `source()` callee, so recovery removes
    // the cycle-forming edges from the reporting index.
    assert_eq!(world.source_targets(FileId(0)), NO_TARGETS);

    world.apply(&replace(FileId(1), "val_b <- 1\n"));

    assert!(!world.any_source_cycle());
    assert_eq!(world.source_targets(FileId(0)), ["w/b.R"]);
}

#[test]
fn test_scenario_mutual_pair_opens_then_closes_again() {
    let initial = scripts(&[
        ("a.R", "source(\"b.R\")\nval_a <- 1\n"),
        ("b.R", "source(\"a.R\")\nval_b <- 1\n"),
    ]);
    let mut world = start(&scenario(initial, Query::Diagnostics(FileId(1))));

    assert!(world.source_cycle_reported(FileId(0)));
    assert!(world.source_cycle_reported(FileId(1)));

    world.apply(&replace(FileId(1), "val_b <- 1\n"));

    assert!(!world.any_source_cycle());
    assert_eq!(world.source_targets(FileId(0)), ["w/b.R"]);

    world.apply(&replace(FileId(1), "source(\"a.R\")\nval_b <- 1\n"));

    assert!(world.source_cycle_reported(FileId(0)));
    assert!(world.source_cycle_reported(FileId(1)));
}

/// Require `cross_file_layers()` to become Salsa's repeated key when entered
/// cold through a package with an `R/` collation.
#[test]
fn test_scenario_package_cold_entry_reaches_cross_file_layers_recovery() {
    let initial = package("mypkg", &["base", "pkga"], &[
        ("R/a.R", "library(pkga)\nsource(\"R/b.R\")\n"),
        ("R/b.R", "library(pkga)\n"),
    ]);
    let _world = start(&scenario(
        initial,
        Query::CrossFileLayers(FileId(1), CollationView::Eager),
    ));

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
    let initial = scripts(&[
        (
            "a.R",
            "source <- function(...) NULL\nsource(\"b.R\")\nval_a <- 1\n",
        ),
        ("b.R", "val_b <- 1\n"),
    ]);
    let world = start(&scenario(initial, Query::Diagnostics(FileId(0))));

    assert_eq!(world.source_targets(FileId(0)), NO_TARGETS);
}
