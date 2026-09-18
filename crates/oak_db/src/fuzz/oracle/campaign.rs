//! Runs comparisons for every seed and round-robin mutated descendants.
//!
//! Mismatches and panics fail immediately. The campaign neither shrinks failures nor maintains its own corpus. Report descendant counts separately because seed coverage cannot show whether mutations reach comparisons.

use std::time::Duration;
use std::time::Instant;

use mutatis::Session;

use crate::fuzz::oracle::compare::Compare;
use crate::fuzz::oracle::compare::Counts;
use crate::fuzz::oracle::compare::Fresh;
use crate::fuzz::scenario::Scenario;
use crate::fuzz::seed_corpus;
use crate::fuzz::Runner;
use crate::fuzz::ScenarioMutator;

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
        summary.seeds.add(&compare(&runner, scenario));
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

        summary.descendants.add(&compare(&runner, &corpus[entry]));
        summary.descendant_scenarios += 1;
    }

    summary.elapsed = started.elapsed();
    summary
}

/// Fails immediately on a mismatch or panic. Check the finding first because a mismatch stops execution before a later panic.
fn compare(runner: &Runner, scenario: &Scenario) -> Counts {
    let mut observer = Compare::new(scenario, Fresh);
    let outcome = runner.check_observed(scenario, &mut observer);

    if let Some(finding) = observer.finding() {
        panic!(
            "semantic mismatch\n{}\n{}\n{}",
            scenario.header(),
            finding.render(),
            scenario.render(),
        );
    }
    match outcome {
        Ok(()) => observer.counts().clone(),
        Err(panic) => panic!("panicked during comparison\n{panic}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mutation makes exact counts unstable. Print coverage so a reached-but-uncompared variant exposes recovery, not scheduling, as the limit.
    #[test]
    fn test_campaign_compares_seeds_and_descendants() {
        let summary = run(0, 1000);
        eprintln!("{}", summary.render());

        assert!(summary.seeds.compared_total() > 0);
        assert!(summary.descendants.compared_total() > 0);
        assert!(summary.descendants.compared_after_edit > 0);
    }
}
