//! Runs comparisons for every seed and round-robin mutated descendants.
//!
//! Mismatches and caught panics save the scenario as JSON for `just
//! fuzz-replay-semantic` before failing. Aborts and hangs never reach
//! reporting, so they rely on the text artifact written before each operation.
//! The campaign neither shrinks failures nor maintains its own corpus. Report
//! descendant counts separately because seed coverage cannot show whether
//! mutations reach comparisons.

use std::path::Path;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use mutatis::Session;

use crate::fuzz::oracle::compare::Compare;
use crate::fuzz::oracle::compare::Counts;
use crate::fuzz::oracle::compare::Fresh;
use crate::fuzz::oracle::compare::Mismatch;
use crate::fuzz::oracle::compare::Reference;
use crate::fuzz::oracle::reduce;
use crate::fuzz::scenario::Scenario;
use crate::fuzz::seed_corpus;
use crate::fuzz::Runner;
use crate::fuzz::ScenarioMutator;

/// The result of one scenario comparison. Reporting stays separate so callers can inspect findings without panicking.
#[derive(Debug)]
pub(crate) enum Outcome {
    Clean(Counts),
    Mismatch(Mismatch),
    Panicked(String),
}

/// Coverage and execution totals from one campaign run.
#[derive(Debug, Default)]
pub(crate) struct Summary {
    pub(crate) scenarios: usize,
    pub(crate) seeds: Counts,
    /// Separate mutation coverage, which confirms that descendants reach comparisons.
    pub(crate) descendants: Counts,
    pub(crate) descendant_scenarios: usize,
    pub(crate) exhausted: usize,
    pub(crate) elapsed: Duration,
}

impl Summary {
    pub(crate) fn render(&self) -> String {
        format!(
            "scenarios {} seeds + {} descendants ({} mutations exhausted) in {:?}\n\
             seeds:       {}\n\
             descendants: {}",
            self.scenarios,
            self.descendant_scenarios,
            self.exhausted,
            self.elapsed,
            render_counts(&self.seeds),
            render_counts(&self.descendants),
        )
    }
}

fn render_counts(counts: &Counts) -> String {
    format!(
        "reached {} (resolve {}/{}, resolve_at {}/{}, package_resolve {}/{}), \
         compared {} (after edit {}), skipped {} historical + {} reference",
        counts.reached_total(),
        counts.resolve.compared,
        counts.resolve.reached,
        counts.resolve_at.compared,
        counts.resolve_at.reached,
        counts.package_resolve.compared,
        counts.package_resolve.reached,
        counts.compared_total(),
        counts.compared_after_edit,
        counts.skipped_historical,
        counts.skipped_reference,
    )
}

/// Compares every seed, then mutates entries round-robin so one seed cannot consume the budget. Mutations update corpus entries in place.
pub(crate) fn run(seed: u64, mutations: usize) -> Summary {
    let runner = Runner::open();
    let started = Instant::now();
    let mut corpus = seed_corpus(seed);
    let mut summary = Summary::default();

    for scenario in &corpus {
        let outcome = compare(&runner, scenario, Fresh);
        summary.seeds.add(&report(&runner, scenario, outcome));
        summary.scenarios += 1;
    }

    let mut session = Session::new().seed(seed);
    for round in 0..mutations {
        let entry = round % corpus.len();
        if session
            .mutate_with(&mut ScenarioMutator, &mut corpus[entry])
            .is_err()
        {
            summary.exhausted += 1;
            continue;
        }

        let outcome = compare(&runner, &corpus[entry], Fresh);
        summary
            .descendants
            .add(&report(&runner, &corpus[entry], outcome));
        summary.descendant_scenarios += 1;
    }

    summary.elapsed = started.elapsed();
    summary
}

/// Replays a saved scenario against `reference`, failing on a mismatch or
/// panic. Accepting a reference lets tests replay an injected mismatch. Prints
/// the comparison counts on a clean result, since "no mismatch" can mean
/// agreement or a recovery skip on every comparison.
pub(crate) fn replay<R: Reference>(scenario: &Scenario, reference: R) {
    let runner = Runner::open();
    let outcome = compare(&runner, scenario, reference);
    let counts = report(&runner, scenario, outcome);
    eprintln!("{}", render_counts(&counts));
}

/// Reduces a saved scenario that already mismatches under `Fresh`, then reports
/// the reduced mismatch and writes replayable JSON.
pub(crate) fn reduce_and_report(scenario: Scenario, seed: u64, shrink_iters: usize) {
    let runner = Runner::open();
    let reduction = reduce::reduce(&runner, scenario, seed, shrink_iters);
    if !reduction.shrink_panics.is_empty() {
        eprintln!(
            "reduction rejected {} shrink-time panic(s) rather than let them replace the mismatch:\n{}",
            reduction.shrink_panics.len(),
            reduction.shrink_panics.join("\n")
        );
    }
    report(
        &runner,
        &reduction.scenario,
        Outcome::Mismatch(reduction.finding),
    );
}

/// Return a mismatch before a later panic because the mismatch stops the comparison.
pub(super) fn compare<R: Reference>(runner: &Runner, scenario: &Scenario, reference: R) -> Outcome {
    let mut observer = Compare::new(scenario, reference);
    let outcome = runner.check_observed(scenario, &mut observer);

    if let Some(finding) = observer.finding() {
        return Outcome::Mismatch(finding.clone());
    }
    match outcome {
        Ok(()) => Outcome::Clean(observer.counts().clone()),
        Err(panic) => Outcome::Panicked(panic),
    }
}

/// Panics for a mismatch or execution panic after saving the scenario as replayable JSON.
fn report(runner: &Runner, scenario: &Scenario, outcome: Outcome) -> Counts {
    match outcome {
        Outcome::Clean(counts) => counts,
        Outcome::Mismatch(finding) => panic!(
            "semantic mismatch\n{}\n{}\n{}{}",
            scenario.header(),
            finding.render(),
            scenario.render(),
            save_replay_input(runner, scenario),
        ),
        Outcome::Panicked(panic) => panic!(
            "panicked during comparison\n{panic}\n{}",
            save_replay_input(runner, scenario)
        ),
    }
}

/// Writes replay JSON beside the runner's non-JSON text artifact. A write
/// failure stays in the failure message so it does not replace the finding.
fn save_replay_input(runner: &Runner, scenario: &Scenario) -> String {
    let path = runner.artifact_path().with_extension("json");
    match write_json(&path, scenario) {
        Ok(()) => format!("replay: just fuzz-replay-semantic {}", shell_quote(&path)),
        Err(err) => format!("no replay input: {err:?}"),
    }
}

fn write_json(path: &Path, scenario: &Scenario) -> anyhow::Result<()> {
    let json = scenario.to_json()?;
    std::fs::write(path, json).with_context(|| format!("failed to write {}", path.display()))
}

/// Quotes `path` for a POSIX shell so spaces and apostrophes remain part of one
/// argument.
fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use stdext::result::ResultExt;

    use super::*;
    use crate::fuzz::corpus;
    use crate::fuzz::oracle::compare::EmptyReference;
    use crate::fuzz::panics::catch_quietly;
    use crate::fuzz::run::Checkpoint;

    /// Mutation makes exact counts unstable. Print coverage so a reached-but-uncompared variant exposes recovery, not scheduling, as the limit.
    #[test]
    fn test_campaign_compares_seeds_and_descendants() {
        let summary = run(0, 1000);
        eprintln!("{}", summary.render());

        assert!(summary.seeds.compared_total() > 0);
        assert!(summary.descendants.compared_total() > 0);
        assert!(summary.descendants.compared_after_edit > 0);
    }

    /// `EmptyReference` supplies a mismatch because fresh databases are expected to agree. This verifies that JSON preserves every comparison input and produces a deterministic finding, not that a real scenario mismatches.
    #[test]
    fn test_a_scenario_restored_from_json_reports_the_same_finding() {
        let scenario = corpus::scenario("rename_and_undo_across_files");
        let restored = from_json(&json(&scenario));

        let runner = Runner::open();
        let direct = finding(compare(&runner, &scenario, EmptyReference));
        let replayed = finding(compare(&runner, &restored, EmptyReference));

        assert_eq!(direct.checkpoint, Checkpoint::ColdEntry);
        assert_eq!(direct.query, scenario.cold_entry);
        assert_eq!(direct.historical.resolved.len(), 1);
        assert_eq!(direct.historical.resolved[0].name, "val_0");
        assert!(direct.reference.resolved.is_empty());

        assert_eq!(direct, replayed);
    }

    /// A reported finding must save replayable JSON because the text artifact cannot be loaded by the replay entry point.
    #[test]
    fn test_a_reported_finding_saves_a_replayable_scenario() {
        let scenario = corpus::scenario("rename_and_undo_across_files");
        let runner = Runner::open();
        let outcome = compare(&runner, &scenario, EmptyReference);

        let path = runner.artifact_path().with_extension("json");
        // Overwrite any earlier artifact so it cannot satisfy the assertions.
        if let Err(err) = std::fs::write(&path, "not a scenario") {
            panic!("failed to prepare {}: {err}", path.display());
        }

        let reported = catch_quietly(|| {
            report(&runner, &scenario, outcome);
        });

        let message = match reported {
            Err(message) => message,
            Ok(()) => panic!("expected the mismatch to be reported"),
        };
        assert!(message.contains("semantic mismatch"));
        assert!(message.contains(&format!("just fuzz-replay-semantic {}", shell_quote(&path))));

        let saved = match std::fs::read_to_string(&path) {
            Ok(saved) => saved,
            Err(err) => panic!("cannot read {}: {err}", path.display()),
        };
        assert_eq!(saved, json(&scenario));
        // `from_json()` runs the validation required by the replay entry point.
        assert_eq!(from_json(&saved).render(), scenario.render());
    }

    /// `Outcome::Panicked` must reach the same save-and-report path as a
    /// mismatch, not a separate one that could skip the JSON.
    #[test]
    fn test_a_panic_saves_a_replayable_scenario() {
        let scenario = corpus::scenario("rename_and_undo_across_files");
        let runner = Runner::open();
        let path = runner.artifact_path().with_extension("json");
        // Overwrite any earlier artifact so it cannot satisfy the assertions.
        if let Err(err) = std::fs::write(&path, "not a scenario") {
            panic!("failed to prepare {}: {err}", path.display());
        }

        let reported = catch_quietly(|| {
            report(&runner, &scenario, Outcome::Panicked("boom".to_string()));
        });

        let message = match reported {
            Err(message) => message,
            Ok(()) => panic!("expected the panic to be reported"),
        };
        assert!(message.contains("panicked during comparison"));
        assert!(message.contains("boom"));
        assert!(message.contains(&format!("just fuzz-replay-semantic {}", shell_quote(&path))));

        let saved = match std::fs::read_to_string(&path) {
            Ok(saved) => saved,
            Err(err) => panic!("cannot read {}: {err}", path.display()),
        };
        assert_eq!(saved, json(&scenario));
    }

    /// A write failure must still report the finding, naming the failure
    /// instead of a replay command that would not work.
    #[test]
    fn test_a_write_failure_is_reported_without_a_replay_hint() {
        let scenario = corpus::scenario("rename_and_undo_across_files");
        let runner = Runner::open();
        let outcome = compare(&runner, &scenario, EmptyReference);

        let path = runner.artifact_path().with_extension("json");
        // A directory at the JSON path makes `std::fs::write()` fail without touching the filesystem's permission bits.
        if let Err(err) = std::fs::remove_file(&path) {
            if err.kind() != std::io::ErrorKind::NotFound {
                panic!("failed to clear {}: {err}", path.display());
            }
        }
        if let Err(err) = std::fs::create_dir(&path) {
            panic!("failed to create {}: {err}", path.display());
        }

        let reported = catch_quietly(|| {
            report(&runner, &scenario, outcome);
        });
        std::fs::remove_dir(&path).log_err();

        let message = match reported {
            Err(message) => message,
            Ok(()) => panic!("expected the mismatch to be reported"),
        };
        assert!(message.contains("semantic mismatch"));
        assert!(message.contains("no replay input:"));
        assert!(!message.contains("just fuzz-replay-semantic"));
    }

    fn finding(outcome: Outcome) -> Mismatch {
        match outcome {
            Outcome::Mismatch(finding) => finding,
            other => panic!("expected a mismatch, got {other:?}"),
        }
    }

    fn json(scenario: &Scenario) -> String {
        match scenario.to_json() {
            Ok(json) => json,
            Err(err) => panic!("{err:?}"),
        }
    }

    fn from_json(json: &str) -> Scenario {
        match Scenario::from_json(json.as_bytes()) {
            Ok(scenario) => scenario,
            Err(err) => panic!("{err:?}"),
        }
    }
}
