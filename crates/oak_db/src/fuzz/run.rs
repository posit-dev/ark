//! Executes each scenario on one database so edits exercise incremental
//! evaluation.
//!
//! [`Runner::check()`] catches property panics so the check can shrink
//! failures. The database is dropped during unwinding before the panic is
//! caught.
//!
//! Database materialization and query dispatch live in `world`; this module
//! owns execution order, tracing, and failure reporting.

mod world;

use std::cell::Cell;
use std::path::Path;

pub(crate) use self::world::World;
use crate::fuzz::artifact::Artifact;
use crate::fuzz::panics::catch_quietly;
use crate::fuzz::panics::install;
use crate::fuzz::panics::Guard;
#[cfg(test)]
use crate::fuzz::scenario::Query;
use crate::fuzz::scenario::Scenario;
#[cfg(test)]
use crate::fuzz::spec::FileId;
use crate::recovery;

/// Owns the failure artifact and the panic hook, so no caller can run a
/// scenario without the context a failure needs.
pub struct Runner {
    artifact: Artifact,
    /// `check()` can only recover a panic's message and location while this guard is alive.
    _hook: Guard,
}

impl Runner {
    pub fn open() -> Runner {
        Runner {
            artifact: Artifact::open(),
            _hook: install(),
        }
    }

    /// Runs `scenario`, returning a panic's message and location on failure.
    pub(crate) fn check(&self, scenario: &Scenario) -> std::result::Result<(), String> {
        run_scenario(scenario, &self.artifact)
    }

    /// Runs `scenario` and lets a panic propagate, which is how a fuzzing
    /// engine learns of a crash. The artifact is written ahead of each
    /// operation, so an abort still names the operation in flight.
    pub fn execute(&self, scenario: &Scenario) {
        run(scenario, &self.artifact, traced())
    }

    /// Re-runs `scenario` with operation tracing, letting a panic propagate.
    pub(crate) fn replay(&self, scenario: &Scenario) {
        run(scenario, &self.artifact, true)
    }

    /// Path of the artifact naming the scenario and operation in flight.
    pub(crate) fn artifact_path(&self) -> &Path {
        self.artifact.path()
    }

    /// Truncates the artifact after a clean run.
    pub(crate) fn clear(&self) {
        self.artifact.clear()
    }
}

/// Preserve the original panic details in case the shrunken failure does not
/// reproduce.
fn run_scenario(scenario: &Scenario, artifact: &Artifact) -> std::result::Result<(), String> {
    catch_quietly(|| run(scenario, artifact, traced()))
        .map_err(|panic| format!("{}\n{panic}", scenario.header()))
}

fn run(scenario: &Scenario, artifact: &Artifact, trace: bool) {
    recovery::reset();
    let report = Report::new(scenario, artifact, trace);

    let mut world = World::materialize(&scenario.initial);
    report.entering("cold entry", &scenario.cold_entry.render());
    world.query(&scenario.cold_entry);
    report.firings();

    for (index, op) in scenario.ops.iter().enumerate() {
        report.entering(&format!("op {index}"), &op.render());
        world.apply(op);
        report.firings();
    }
}

fn traced() -> bool {
    std::env::var_os("OAK_FUZZ_TRACE").is_some()
}

/// Print the scenario before execution because hangs and aborts do not unwind.
#[cfg(test)]
pub(crate) fn start(scenario: &Scenario) -> World {
    recovery::reset();
    eprintln!("{}", scenario.header());
    eprint!("{}", scenario.render());
    let world = World::materialize(&scenario.initial);
    world.query(&scenario.cold_entry);
    world
}

/// Record each operation before it runs so hangs and aborts leave an artifact.
/// Tracing also prints this context during replay and `OAK_FUZZ_TRACE=1` runs.
struct Report<'scenario> {
    trace: bool,
    /// Recovery firings already printed while tracing.
    printed_firings: Cell<usize>,
    artifact: &'scenario Artifact,
}

impl Report<'_> {
    fn new<'scenario>(
        scenario: &'scenario Scenario,
        artifact: &'scenario Artifact,
        trace: bool,
    ) -> Report<'scenario> {
        if trace {
            eprintln!("{}", scenario.header());
            eprint!("{}", scenario.render());
        }
        artifact.reset(format!("{}\n{}", scenario.header(), scenario.render()));
        Report {
            trace,
            printed_firings: Cell::new(0),
            artifact,
        }
    }

    fn entering(&self, position: &str, operation: &str) {
        let current = format!("{position}: {operation}");
        if self.trace {
            eprintln!("  {current}");
        }
        self.artifact.entering(&current);
    }

    fn firings(&self) {
        if !self.trace {
            return;
        }
        let fired = recovery::fired();
        for entry in &fired[self.printed_firings.get()..] {
            eprintln!("      recovered: {entry}");
        }
        self.printed_firings.set(fired.len());
    }
}

#[cfg(test)]
mod tests {
    use std::panic::AssertUnwindSafe;

    use super::*;
    use crate::fuzz::corpus;

    /// The artifact must identify the active operation even without unwinding.
    #[test]
    fn test_artifact_records_scenario_and_failing_operation() {
        let scenario = corpus::case("acyclic_pair_closes_then_reopens");
        let artifact = Artifact::open();
        let operation = scenario.ops[0].render();

        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let report = Report::new(&scenario, &artifact, false);
            report.entering("op 0", &operation);
            panic!("simulated hang point");
        }));
        assert!(outcome.is_err());

        let content = std::fs::read_to_string(artifact.path()).unwrap();
        let expected = format!(
            "{}\n{}  current: op 0: {operation}\n",
            scenario.header(),
            scenario.render()
        );
        assert_eq!(content, expected);
    }

    /// A fuzzing engine detects a crash only if the panic reaches it, so
    /// `execute()` must not swallow what `check()` deliberately catches.
    #[test]
    fn test_execute_lets_a_panic_propagate() {
        let mut scenario = corpus::case("acyclic_pair_closes_then_reopens");
        scenario.cold_entry = Query::Diagnostics(FileId(9));

        let runner = Runner::open();
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| runner.execute(&scenario)));

        assert!(outcome.is_err());
    }

    /// `Runner::open()` installs the panic hook itself, so `check()` must recover
    /// the real panic message without a separately installed hook.
    #[test]
    fn test_check_recovers_panic_without_separately_installed_hook() {
        let mut scenario = corpus::case("acyclic_pair_closes_then_reopens");
        scenario.cold_entry = Query::Diagnostics(FileId(9));

        let runner = Runner::open();
        let outcome = runner.check(&scenario);

        assert!(outcome.is_err());
        let message = outcome.unwrap_err();
        assert!(message.contains("index out of bounds"));
        assert!(!message.contains("panicked without reaching the panic hook"));
    }
}
