//! Compares each scheduled resolution result with a freshly materialized database.
//!
//! Compare only the prefix before either execution recovers. Salsa memoizes a cycle fallback, so later checkpoints can reuse it without another recovery handler firing. The first recovery therefore disables comparison for the rest of the scenario, while panic and hang checks continue.
//!
//! Attribute reference recovery from log boundaries captured immediately around its execution. Any other recovery-log growth belongs to the historical database.

mod tests;

use std::ops::Range;

use crate::fuzz::oracle::observe::observe;
use crate::fuzz::oracle::observe::Observation;
use crate::fuzz::run::Checkpoint;
use crate::fuzz::run::Observe;
use crate::fuzz::run::Observed;
use crate::fuzz::run::World;
use crate::fuzz::scenario::Op;
use crate::fuzz::scenario::Query;
use crate::fuzz::scenario::Scenario;
use crate::fuzz::spec::WorkspaceSpec;
use crate::recovery;
use crate::Db;
use crate::Definition;

/// Resolution checkpoints reached by a campaign run. Comparisons and skips are disjoint, and together account for every reached resolution checkpoint.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    pub(crate) resolve: usize,
    pub(crate) resolve_at: usize,
    pub(crate) package_resolve: usize,
    pub(crate) compared: usize,
    /// Comparisons after an edit, where incremental evaluation can differ from a fresh database.
    pub(crate) compared_after_edit: usize,
    pub(crate) skipped_historical: usize,
    pub(crate) skipped_reference: usize,
}

impl Counts {
    fn reached(&mut self, query: &Query) {
        match query {
            Query::Resolve(..) => self.resolve += 1,
            Query::ResolveAt(..) => self.resolve_at += 1,
            Query::PackageResolve(..) => self.package_resolve += 1,
            // Only resolution variants call `Observe::resolved()`.
            _ => {},
        }
    }
}

/// A disagreement between the two executions at one checkpoint.
#[derive(Clone, Debug)]
pub(crate) struct Mismatch {
    pub(crate) checkpoint: Checkpoint,
    pub(crate) query: Query,
    pub(crate) historical: Observation,
    pub(crate) reference: Observation,
}

/// Produces a comparison's reference answer. Test implementations model stale results and reference recovery without depending on Salsa cache timing.
pub(crate) trait Reference {
    /// Returns the reference observation and any recovery-log interval it appended.
    fn observe(
        &mut self,
        spec: &WorkspaceSpec,
        query: &Query,
    ) -> (Observation, Option<Range<usize>>);
}

/// Resolves the same query in a freshly materialized workspace.
pub(crate) struct Fresh;

impl Reference for Fresh {
    fn observe(
        &mut self,
        spec: &WorkspaceSpec,
        query: &Query,
    ) -> (Observation, Option<Range<usize>>) {
        let world = World::materialize(spec);
        let mut collect = Collect::default();

        let start = recovery::fired().len();
        world.query(query, &mut collect);
        let end = recovery::fired().len();

        let observation = match collect.observation {
            Some(observation) => observation,
            // The identical query variant must invoke `Collect::resolved()`.
            None => panic!("harness bug: reference did not observe {}", query.render()),
        };
        (observation, (start < end).then_some(start..end))
    }
}

/// Captures a reference result without starting another comparison.
#[derive(Default)]
pub(crate) struct Collect {
    pub(crate) observation: Option<Observation>,
}

impl Observe for Collect {
    fn resolved(
        &mut self,
        db: &dyn Db,
        _spec: &WorkspaceSpec,
        _query: &Query,
        definitions: &[Definition<'_>],
    ) -> Observed {
        self.observation = Some(observe(db, definitions));
        Observed::proceed(None)
    }
}

pub(crate) struct Compare<'scenario, R: Reference> {
    scenario: &'scenario Scenario,
    reference: R,
    /// Log entries already attributed to one side or the other.
    accounted: usize,
    historical_recovered: bool,
    reference_recovered: bool,
    edited: bool,
    checkpoint: Checkpoint,
    counts: Counts,
    finding: Option<Mismatch>,
}

impl<'scenario, R: Reference> Compare<'scenario, R> {
    pub(crate) fn new(scenario: &'scenario Scenario, reference: R) -> Compare<'scenario, R> {
        Compare {
            scenario,
            reference,
            accounted: 0,
            historical_recovered: false,
            reference_recovered: false,
            edited: false,
            checkpoint: Checkpoint::ColdEntry,
            counts: Counts::default(),
            finding: None,
        }
    }

    pub(crate) fn counts(&self) -> &Counts {
        &self.counts
    }

    /// Mismatch that stopped the scenario, if any. A mismatching checkpoint cannot also report a later panic.
    pub(crate) fn finding(&self) -> Option<&Mismatch> {
        self.finding.as_ref()
    }

    pub(crate) fn historical_recovered(&self) -> bool {
        self.historical_recovered
    }

    /// Attributes recovery-log growth since the last boundary to the historical execution.
    fn account_historical(&mut self) {
        let fired = recovery::fired().len();
        if fired > self.accounted {
            self.historical_recovered = true;
            self.accounted = fired;
        }
    }
}

impl<R: Reference> Observe for Compare<'_, R> {
    /// Attribute recovery after the last resolution checkpoint, which no later `resolved()` call can observe.
    fn finished(&mut self) {
        self.account_historical();
    }

    fn entering(&mut self, checkpoint: Checkpoint) {
        self.checkpoint = checkpoint;

        // Edits run after `entering()` and never call `resolved()`, so their next resolution checkpoint is the first post-edit comparison.
        if let Checkpoint::Op(index) = checkpoint {
            if matches!(self.scenario.ops.get(index), Some(Op::Edit(_))) {
                self.edited = true;
            }
        }
    }

    fn resolved(
        &mut self,
        db: &dyn Db,
        spec: &WorkspaceSpec,
        query: &Query,
        definitions: &[Definition<'_>],
    ) -> Observed {
        self.counts.reached(query);

        // Capture the historical result before reference work can affect recovery state.
        let historical = observe(db, definitions);

        self.account_historical();
        if self.historical_recovered {
            self.counts.skipped_historical += 1;
            return Observed::proceed(None);
        }
        if self.reference_recovered {
            self.counts.skipped_reference += 1;
            return Observed::proceed(None);
        }

        let (reference, interval) = self.reference.observe(spec, query);
        if let Some(interval) = interval {
            self.reference_recovered = true;
            self.accounted = interval.end;
            self.counts.skipped_reference += 1;
            return Observed::proceed(Some(interval));
        }

        self.counts.compared += 1;
        if self.edited {
            self.counts.compared_after_edit += 1;
        }

        if historical == reference {
            return Observed::proceed(None);
        }

        self.finding = Some(Mismatch {
            checkpoint: self.checkpoint,
            query: query.clone(),
            historical,
            reference,
        });
        Observed::stop(None)
    }
}
