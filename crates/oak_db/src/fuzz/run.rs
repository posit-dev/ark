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
use std::ops::ControlFlow;
use std::ops::Range;
use std::path::Path;

pub(crate) use self::world::World;
use crate::fuzz::artifact::Artifact;
use crate::fuzz::panics::catch_quietly;
use crate::fuzz::panics::install;
use crate::fuzz::panics::Guard;
use crate::fuzz::scenario::Query;
use crate::fuzz::scenario::Scenario;
#[cfg(test)]
use crate::fuzz::spec::FileId;
use crate::fuzz::spec::WorkspaceSpec;
use crate::recovery;
use crate::Db;
use crate::Definition;

/// Observes definitions after their scheduled resolution query runs on the historical database.
///
/// Implementations may run a reference query that appends to the process-global recovery log in [`crate::recovery`]. Return that interval so [`Report`] labels its firings without resetting evidence needed by either execution.
pub(crate) trait Observe {
    /// Announces each checkpoint before it runs. Edits never call [`Observe::resolved()`], so observers identify post-edit resolution checkpoints here.
    fn entering(&mut self, _checkpoint: Checkpoint) {}

    fn resolved(
        &mut self,
        db: &dyn Db,
        spec: &WorkspaceSpec,
        query: &Query,
        definitions: &[Definition<'_>],
    ) -> Observed;

    /// Announces completion after exhausting operations or stopping at a finding. Recovery after the last observed query reaches the observer only here.
    fn finished(&mut self) {}
}

/// Scenario position used in artifacts and mismatch reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Checkpoint {
    #[default]
    ColdEntry,
    Op(usize),
}

impl Checkpoint {
    pub(crate) fn render(&self) -> String {
        match self {
            Checkpoint::ColdEntry => "cold entry".to_string(),
            Checkpoint::Op(index) => format!("op {index}"),
        }
    }
}

/// Observer output for one scheduled query.
pub(crate) struct Observed {
    /// Interval of recovery-log entries appended by the observer's reference execution.
    pub(crate) reference: Option<Range<usize>>,
    /// `Break` ends the scenario after a valid finding. Continuing could obscure its trace with a later panic or recovery.
    pub(crate) flow: ControlFlow<()>,
}

impl Observed {
    pub(crate) fn proceed(reference: Option<Range<usize>>) -> Observed {
        Observed {
            reference,
            flow: ControlFlow::Continue(()),
        }
    }

    pub(crate) fn stop(reference: Option<Range<usize>>) -> Observed {
        Observed {
            reference,
            flow: ControlFlow::Break(()),
        }
    }
}

/// Disables resolution observation for the unrestricted fuzz campaign.
pub(crate) struct Unobserved;

impl Observe for Unobserved {
    fn resolved(
        &mut self,
        _db: &dyn Db,
        _spec: &WorkspaceSpec,
        _query: &Query,
        _definitions: &[Definition<'_>],
    ) -> Observed {
        Observed::proceed(None)
    }
}

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
        run_scenario(scenario, &self.artifact, &mut Unobserved)
    }

    /// Like [`Runner::check()`], while passing each scheduled resolution result to `observer`. On panic, `observer` retains results recorded before unwinding.
    #[cfg(test)]
    pub(crate) fn check_observed(
        &self,
        scenario: &Scenario,
        observer: &mut dyn Observe,
    ) -> std::result::Result<(), String> {
        run_scenario(scenario, &self.artifact, observer)
    }

    /// Runs `scenario` and lets a panic propagate, which is how a fuzzing
    /// engine learns of a crash. The artifact is written ahead of each
    /// operation, so an abort still names the operation in flight.
    pub(crate) fn execute(&self, scenario: &Scenario) {
        run(scenario, &self.artifact, traced(), &mut Unobserved)
    }

    /// Re-runs `scenario` with operation tracing, letting a panic propagate.
    pub(crate) fn replay(&self, scenario: &Scenario) {
        run(scenario, &self.artifact, true, &mut Unobserved)
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
fn run_scenario(
    scenario: &Scenario,
    artifact: &Artifact,
    observer: &mut dyn Observe,
) -> std::result::Result<(), String> {
    catch_quietly(|| run(scenario, artifact, traced(), observer))
        .map_err(|panic| format!("{}\n{panic}", scenario.header()))
}

fn run(scenario: &Scenario, artifact: &Artifact, trace: bool, observer: &mut dyn Observe) {
    recovery::reset();
    let report = Report::new(scenario, artifact, trace);

    execute(scenario, &report, observer);
    observer.finished();
}

/// Returns before [`Observe::finished()`] so completion handling runs for every exit.
fn execute(scenario: &Scenario, report: &Report<'_>, observer: &mut dyn Observe) {
    let mut world = World::materialize(&scenario.initial);
    report.entering(
        &Checkpoint::ColdEntry.render(),
        &scenario.cold_entry.render(),
    );
    observer.entering(Checkpoint::ColdEntry);
    // Report the checkpoint's reference firings before stopping so they remain in the trace.
    let observed = world.query(&scenario.cold_entry, observer);
    report.firings(observed.reference);
    if observed.flow.is_break() {
        return;
    }

    for (index, op) in scenario.ops.iter().enumerate() {
        let checkpoint = Checkpoint::Op(index);
        report.entering(&checkpoint.render(), &op.render());
        observer.entering(checkpoint);
        let observed = world.apply(op, observer);
        report.firings(observed.reference);
        if observed.flow.is_break() {
            return;
        }
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
    world.query(&scenario.cold_entry, &mut Unobserved);
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
            eprintln!("scenario: {}", scenario.header());
            eprint!("{}", scenario.render());
            eprintln!("trace:");
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

    /// Label only recovery firings from the observer's reference execution. This preserves unobserved trace output while advancing the cursor past all firings.
    fn firings(&self, reference: Option<Range<usize>>) {
        if !self.trace {
            return;
        }
        let fired = recovery::fired();
        for (index, entry) in fired.iter().enumerate().skip(self.printed_firings.get()) {
            eprintln!(
                "      recovered{}: {entry}",
                firing_origin(&reference, index)
            );
        }
        self.printed_firings.set(fired.len());
    }
}

/// Leaves historical firings unlabelled to preserve unobserved trace output.
fn firing_origin(reference: &Option<Range<usize>>, index: usize) -> &'static str {
    match reference {
        Some(interval) if interval.contains(&index) => " (reference)",
        _ => "",
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
        let scenario = corpus::scenario("acyclic_pair_closes_then_reopens");
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
        let mut scenario = corpus::scenario("acyclic_pair_closes_then_reopens");
        scenario.cold_entry = Query::Diagnostics(FileId(9));

        let runner = Runner::open();
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| runner.execute(&scenario)));

        assert!(outcome.is_err());
    }

    /// `Runner::open()` installs the panic hook itself, so `check()` must recover
    /// the real panic message without a separately installed hook.
    #[test]
    fn test_check_recovers_panic_without_separately_installed_hook() {
        let mut scenario = corpus::scenario("acyclic_pair_closes_then_reopens");
        scenario.cold_entry = Query::Diagnostics(FileId(9));

        let runner = Runner::open();
        let outcome = runner.check(&scenario);

        assert!(outcome.is_err());
        let message = outcome.unwrap_err();
        assert!(message.contains("index out of bounds"));
        assert!(!message.contains("panicked without reaching the panic hook"));
    }

    #[test]
    fn test_only_reference_firings_carry_a_label() {
        let reference = Some(2..4);

        assert_eq!(firing_origin(&reference, 1), "");
        assert_eq!(firing_origin(&reference, 2), " (reference)");
        assert_eq!(firing_origin(&reference, 3), " (reference)");
        assert_eq!(firing_origin(&reference, 4), "");
        assert_eq!(firing_origin(&None, 2), "");
    }
}
