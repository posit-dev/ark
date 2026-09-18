//! Tests comparison boundaries around recovery.

use std::ops::Range;

use oak_semantic::fuzz::Program;
use oak_semantic::fuzz::Stmt;

use super::*;
use crate::fuzz::build::binding;
use crate::fuzz::build::source;
use crate::fuzz::scenario::Edit;
use crate::fuzz::spec::FileId;
use crate::fuzz::spec::FileSpec;
use crate::fuzz::spec::Owner;
use crate::fuzz::Runner;

fn script(path: &str, statements: Vec<Stmt>) -> FileSpec {
    FileSpec {
        owner: Owner::Script,
        path: path.to_string(),
        program: Program { statements },
    }
}

fn workspace(files: Vec<FileSpec>) -> WorkspaceSpec {
    WorkspaceSpec {
        installed: vec!["base".to_string()],
        packages: Vec::new(),
        files,
    }
}

/// An acyclic source graph for comparisons without recovery.
fn acyclic_pair() -> WorkspaceSpec {
    workspace(vec![
        script("a.R", vec![binding("val_0")]),
        script("b.R", vec![source("a.R"), binding("val_1")]),
    ])
}

/// A source cycle that makes resolution re-enter its own key.
fn mutual_pair() -> WorkspaceSpec {
    workspace(vec![
        script("a.R", vec![source("b.R"), binding("val_0")]),
        script("b.R", vec![source("a.R"), binding("val_1")]),
    ])
}

fn resolve(file: usize, name: &str) -> Query {
    Query::Resolve(FileId(file), name.to_string())
}

fn edit(file: usize, statements: Vec<Stmt>) -> Op {
    Op::Edit(Edit {
        file: FileId(file),
        program: Program { statements },
    })
}

fn run<R: Reference>(scenario: &Scenario, reference: R) -> Compare<'_, R> {
    let runner = Runner::open();
    let mut observer = Compare::new(scenario, reference);
    let outcome = runner.check_observed(scenario, &mut observer);

    assert_eq!(outcome, Ok(()));
    observer
}

/// Simulates a stale reference by returning no definitions.
struct EmptyReference;

impl Reference for EmptyReference {
    fn observe(
        &mut self,
        _spec: &WorkspaceSpec,
        _query: &Query,
    ) -> (Observation, Option<Range<usize>>) {
        (Observation { resolved: vec![] }, None)
    }
}

/// Simulates recovery on its first execution without depending on Salsa cache state.
#[derive(Default)]
struct RecoveringReference {
    executions: usize,
}

impl Reference for RecoveringReference {
    fn observe(
        &mut self,
        _spec: &WorkspaceSpec,
        _query: &Query,
    ) -> (Observation, Option<Range<usize>>) {
        self.executions += 1;
        (Observation { resolved: vec![] }, Some(0..1))
    }
}

/// Produces real reference recovery-log entries from a cyclic workspace while the historical database remains acyclic.
struct CyclicReference;

impl Reference for CyclicReference {
    fn observe(
        &mut self,
        _spec: &WorkspaceSpec,
        query: &Query,
    ) -> (Observation, Option<Range<usize>>) {
        Fresh.observe(&mutual_pair(), query)
    }
}

#[test]
fn test_compares_a_resolution_cold_entry() {
    let scenario = Scenario::cold(acyclic_pair(), resolve(1, "val_0"), vec![]);

    let observer = run(&scenario, Fresh);

    assert_eq!(observer.counts().resolve.reached, 1);
    assert_eq!(observer.counts().compared_total(), 1);
    assert_eq!(observer.counts().skipped_historical, 0);
    assert_eq!(observer.counts().skipped_reference, 0);
    assert!(observer.finding().is_none());
    assert!(!observer.historical_recovered());
}

/// Empty resolution results are compared rather than skipped.
#[test]
fn test_compares_empty_results() {
    let ops = vec![Op::Query(resolve(1, "absent"))];
    let scenario = Scenario::cold(acyclic_pair(), resolve(1, "absent"), ops);

    let observer = run(&scenario, Fresh);

    assert_eq!(observer.counts().resolve.reached, 2);
    assert_eq!(observer.counts().compared_total(), 2);
    assert!(observer.finding().is_none());
}

#[test]
fn test_counts_a_comparison_after_an_edit() {
    let ops = vec![
        edit(0, vec![binding("val_2")]),
        Op::Query(resolve(1, "val_2")),
    ];
    let scenario = Scenario::cold(acyclic_pair(), resolve(1, "val_0"), ops);

    let observer = run(&scenario, Fresh);

    assert_eq!(observer.counts().compared_total(), 2);
    assert_eq!(observer.counts().compared_after_edit, 1);
}

/// Recovery in a non-resolution cold entry is attributed at the next resolution checkpoint.
#[test]
fn test_recovery_in_another_query_skips_a_later_comparison() {
    let ops = vec![Op::Query(resolve(0, "val_0"))];
    let scenario = Scenario::cold(mutual_pair(), Query::Diagnostics(FileId(0)), ops);

    let observer = run(&scenario, Fresh);

    assert!(observer.historical_recovered());
    assert_eq!(observer.counts().resolve.reached, 1);
    assert_eq!(observer.counts().compared_total(), 0);
    assert_eq!(observer.counts().skipped_historical, 1);
}

#[test]
fn test_recovery_during_the_query_skips_its_own_comparison() {
    let scenario = Scenario::cold(mutual_pair(), resolve(0, "val_1"), vec![]);

    let observer = run(&scenario, Fresh);

    assert!(observer.historical_recovered());
    assert_eq!(observer.counts().resolve.reached, 1);
    assert_eq!(observer.counts().compared_total(), 0);
    assert_eq!(observer.counts().skipped_historical, 1);
}

/// Reference recovery disables later comparisons without running another reference.
#[test]
fn test_reference_recovery_skips_every_later_comparison() {
    let ops = vec![Op::Query(resolve(1, "val_0"))];
    let scenario = Scenario::cold(acyclic_pair(), resolve(1, "val_0"), ops);

    let observer = run(&scenario, RecoveringReference::default());

    assert_eq!(observer.counts().resolve.reached, 2);
    assert_eq!(observer.counts().compared_total(), 0);
    assert_eq!(observer.counts().skipped_reference, 2);
    assert_eq!(observer.reference.executions, 1);
}

/// `finished()` must attribute recovery after the last resolution checkpoint.
#[test]
fn test_recovery_in_the_last_operation_is_attributed() {
    let ops = vec![
        edit(0, vec![source("b.R"), binding("val_0")]),
        Op::Query(Query::Diagnostics(FileId(0))),
    ];
    let scenario = Scenario::cold(acyclic_pair(), resolve(1, "val_0"), ops);

    let observer = run(&scenario, Fresh);

    assert!(observer.historical_recovered());
    // The comparison before the cycle closed remains valid.
    assert_eq!(observer.counts().compared_total(), 1);
    assert!(observer.finding().is_none());
}

/// Reference and historical firings share one log. Reference intervals must not mark history, while later historical growth must.
#[test]
fn test_reference_firings_are_not_charged_to_history() {
    let runner = Runner::open();

    // Only the reference recovers.
    let reference_only = Scenario::cold(acyclic_pair(), resolve(1, "val_0"), vec![]);
    let mut observer = Compare::new(&reference_only, CyclicReference);
    assert_eq!(
        runner.check_observed(&reference_only, &mut observer),
        Ok(())
    );

    assert!(!recovery::fired().is_empty());
    assert!(!observer.historical_recovered());
    assert_eq!(observer.counts().skipped_reference, 1);
    assert_eq!(observer.counts().compared_total(), 0);

    // Historical recovery must register after existing reference entries.
    let ops = vec![
        edit(0, vec![source("b.R"), binding("val_0")]),
        Op::Query(Query::Diagnostics(FileId(0))),
    ];
    let scenario = Scenario::cold(acyclic_pair(), resolve(1, "val_0"), ops);
    let mut observer = Compare::new(&scenario, CyclicReference);
    assert_eq!(runner.check_observed(&scenario, &mut observer), Ok(()));

    assert!(observer.historical_recovered());
    assert_eq!(observer.counts().skipped_reference, 1);
}

/// Stopping at the mismatch prevents a later cycle-closing edit from obscuring the finding.
#[test]
fn test_a_mismatch_stops_the_scenario() {
    let ops = vec![
        edit(0, vec![source("b.R"), binding("val_0")]),
        Op::Query(resolve(0, "val_1")),
    ];
    let scenario = Scenario::cold(acyclic_pair(), resolve(1, "val_0"), ops);

    let observer = run(&scenario, EmptyReference);

    let finding = match observer.finding() {
        Some(finding) => finding,
        None => panic!("expected a mismatch"),
    };
    assert_eq!(finding.checkpoint, Checkpoint::ColdEntry);
    assert_eq!(finding.query, resolve(1, "val_0"));
    assert_eq!(finding.historical.resolved.len(), 1);
    assert_eq!(finding.historical.resolved[0].name, "val_0");
    assert!(finding.reference.resolved.is_empty());

    // The count confirms the cycle-closing edit did not run.
    assert_eq!(observer.counts().resolve.reached, 1);
    assert!(!observer.historical_recovered());
}
