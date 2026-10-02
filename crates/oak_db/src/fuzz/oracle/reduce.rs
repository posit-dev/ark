//! Shrinks a mismatching scenario while preserving its mismatch signature.
//!
//! [`Check::shrink()`] treats any `Err` as a valid failure. The input's
//! [`Signature`] is fixed before shrinking, so only candidates with that same
//! signature are accepted. Panics and differently signed mismatches are
//! rejected.
//!
//! A matching signature preserves only query identity and difference class. It
//! does not prove that candidates share a root cause.

use mutatis::check::Check;
use mutatis::check::CheckError;

use super::campaign::compare;
use super::campaign::Outcome;
use crate::fuzz::oracle::compare::Fresh;
use crate::fuzz::oracle::compare::Mismatch;
use crate::fuzz::oracle::compare::Reference;
use crate::fuzz::oracle::observe::Observation;
use crate::fuzz::run::Runner;
use crate::fuzz::scenario::Query;
use crate::fuzz::scenario::Scenario;
use crate::fuzz::scenario::Site;
use crate::fuzz::spec::WorkspaceSpec;
use crate::fuzz::ScenarioMutator;
use crate::NamespaceVisibility;

/// A smaller scenario whose mismatch has the input scenario's [`Signature`].
pub(super) struct Reduction {
    pub(super) scenario: Scenario,
    pub(super) finding: Mismatch,
    /// Panics from shrink candidates. They cannot replace the target, but are
    /// still reported.
    pub(super) shrink_panics: Vec<String>,
}

/// Reduces a scenario that already mismatches under [`Fresh`].
///
/// Panics if the input does not mismatch, if the input itself panics, or if
/// the reduced scenario's mismatch has a different signature.
pub(super) fn reduce(
    runner: &Runner,
    scenario: Scenario,
    seed: u64,
    shrink_iters: usize,
) -> Reduction {
    reduce_with(runner, scenario, seed, shrink_iters, || Fresh)
}

/// Creates a reference for each candidate so state from one evaluation cannot
/// affect another.
fn reduce_with<R: Reference>(
    runner: &Runner,
    scenario: Scenario,
    seed: u64,
    shrink_iters: usize,
    mut reference: impl FnMut() -> R,
) -> Reduction {
    let signature = match compare(runner, &scenario, reference()) {
        Outcome::Clean(_) => panic!("input does not reproduce a recovery-free semantic mismatch"),
        Outcome::Panicked(message) => {
            panic!("input panicked before ever producing a mismatch to reduce: {message}")
        },
        Outcome::Mismatch(finding) => Signature::of(&finding, &scenario.initial),
    };

    let mut shrink_panics = Vec::new();

    let outcome = Check::new()
        .iters(0)
        .shrink_iters(shrink_iters)
        .seed(seed)
        .run_with(ScenarioMutator, [scenario], |candidate| {
            let outcome = compare(runner, candidate, reference());
            if let Outcome::Panicked(message) = &outcome {
                shrink_panics.push(message.clone());
            }
            match matching_mismatch(outcome, &candidate.initial, &signature) {
                Some(finding) => Err(finding.render()),
                None => Ok(()),
            }
        });

    let failure = match outcome {
        Err(CheckError::Failed(failure)) => failure,
        Err(other) => panic!("mutatis check error: {other}"),
        Ok(()) => {
            panic!("harness bug: the input matches its own signature but the shrinker reported no failure")
        },
    };

    match compare(runner, &failure.value, reference()) {
        Outcome::Mismatch(finding) => {
            assert_eq!(
                Signature::of(&finding, &failure.value.initial),
                signature,
                "reduced scenario's mismatch signature no longer matches the input it was reduced from"
            );
            Reduction {
                scenario: failure.value,
                finding,
                shrink_panics,
            }
        },
        other => panic!("reduced scenario no longer mismatches: {other:?}"),
    }
}

/// Returns the mismatch only when it has `target`'s signature. All other
/// outcomes are rejected.
fn matching_mismatch(
    outcome: Outcome,
    spec: &WorkspaceSpec,
    target: &Signature,
) -> Option<Mismatch> {
    match outcome {
        Outcome::Mismatch(finding) if Signature::of(&finding, spec) == *target => Some(finding),
        _ => None,
    }
}

/// Identifies a mismatch well enough to survive reduction, deliberately
/// excluding the operation index so shrinking stays free to remove and
/// retarget operations.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Signature {
    target: Target,
    difference: Difference,
}

/// The query's logical identity, resolved against the scenario that produced
/// it. `FileId` and `PackageId` are indices into vectors that shrinking can
/// renumber by removing an earlier entry, so a stable identity needs the
/// logical path or package name instead: a removal that only renumbers the
/// same file keeps its path, while a removal that retargets a query to a
/// different file changes it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Resolve {
        path: String,
        name: String,
    },
    ResolveAt {
        path: String,
        site: Site,
    },
    PackageResolve {
        package: String,
        name: String,
        visibility: NamespaceVisibility,
    },
}

impl Signature {
    fn of(finding: &Mismatch, spec: &WorkspaceSpec) -> Signature {
        let target = match &finding.query {
            Query::Resolve(file, name) => Target::Resolve {
                path: spec.absolute_path(*file),
                name: name.clone(),
            },
            Query::ResolveAt(file, site) => Target::ResolveAt {
                path: spec.absolute_path(*file),
                site: *site,
            },
            Query::PackageResolve(package, name, visibility) => Target::PackageResolve {
                package: spec.packages[package.0].name.clone(),
                name: name.clone(),
                visibility: *visibility,
            },
            other => panic!("harness bug: {other:?} is not a resolution query"),
        };
        Signature {
            target,
            difference: Difference::of(&finding.historical, &finding.reference),
        }
    }
}

/// A coarse class of disagreement, ignoring the resolved definitions'
/// specific ranges and kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Difference {
    HistoricalOnly,
    ReferenceOnly,
    BothPresent,
}

impl Difference {
    fn of(historical: &Observation, reference: &Observation) -> Difference {
        match (
            historical.resolved.is_empty(),
            reference.resolved.is_empty(),
        ) {
            (false, true) => Difference::HistoricalOnly,
            (true, false) => Difference::ReferenceOnly,
            _ => Difference::BothPresent,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use oak_semantic::fuzz::Program;

    use super::*;
    use crate::fuzz::corpus;
    use crate::fuzz::oracle::compare::EmptyReference;
    use crate::fuzz::run::Checkpoint;
    use crate::fuzz::spec::FileId;
    use crate::fuzz::spec::FileSpec;
    use crate::fuzz::spec::Owner;

    fn workspace(paths: &[&str]) -> WorkspaceSpec {
        WorkspaceSpec {
            installed: Vec::new(),
            packages: Vec::new(),
            files: paths
                .iter()
                .map(|path| FileSpec {
                    owner: Owner::Script,
                    path: path.to_string(),
                    program: Program {
                        statements: Vec::new(),
                    },
                })
                .collect(),
        }
    }

    fn mismatch(file: usize, name: &str, historical_present: bool) -> Mismatch {
        let present = Observation {
            resolved: vec![crate::fuzz::oracle::observe::Resolved {
                path: "w/a.R".to_string(),
                name: name.to_string(),
                kind: crate::fuzz::oracle::observe::KindTag::Assignment,
                range: Default::default(),
                forward: None,
            }],
        };
        let absent = Observation { resolved: vec![] };
        Mismatch {
            checkpoint: Checkpoint::ColdEntry,
            query: Query::Resolve(FileId(file), name.to_string()),
            historical: if historical_present {
                present.clone()
            } else {
                absent.clone()
            },
            reference: if historical_present { absent } else { present },
        }
    }

    fn spec_with_one_file() -> WorkspaceSpec {
        corpus::scenario("rename_and_undo_across_files").initial
    }

    #[test]
    fn test_matching_mismatch_rejects_clean() {
        let spec = spec_with_one_file();
        let target = Signature::of(&mismatch(1, "val_0", true), &spec);

        let found = matching_mismatch(Outcome::Clean(Default::default()), &spec, &target);

        assert!(found.is_none());
    }

    #[test]
    fn test_matching_mismatch_rejects_panic() {
        let spec = spec_with_one_file();
        let target = Signature::of(&mismatch(1, "val_0", true), &spec);

        let found = matching_mismatch(Outcome::Panicked("boom".to_string()), &spec, &target);

        assert!(found.is_none());
    }

    #[test]
    fn test_matching_mismatch_accepts_the_same_signature() {
        let spec = spec_with_one_file();
        let target = Signature::of(&mismatch(1, "val_0", true), &spec);

        let found = matching_mismatch(
            Outcome::Mismatch(mismatch(1, "val_0", true)),
            &spec,
            &target,
        );

        assert!(found.is_some());
    }

    #[test]
    fn test_matching_mismatch_rejects_a_different_signature() {
        let spec = spec_with_one_file();
        let target = Signature::of(&mismatch(1, "val_0", true), &spec);

        let found = matching_mismatch(
            Outcome::Mismatch(mismatch(1, "val_3", true)),
            &spec,
            &target,
        );

        assert!(found.is_none());
    }

    /// `rename_and_undo_across_files` mismatches at the cold entry under
    /// `EmptyReference` regardless of its later operations, so shrinking is
    /// free to remove all of them while preserving the same signature.
    #[test]
    fn test_reduce_shrinks_while_preserving_the_mismatch_signature() {
        let scenario = corpus::scenario("rename_and_undo_across_files");
        let runner = Runner::open();
        let original = match compare(&runner, &scenario, EmptyReference) {
            Outcome::Mismatch(finding) => finding,
            other => panic!("expected a mismatch, got {other:?}"),
        };
        // `original`'s `FileId`s are relative to `scenario.initial`, not the
        // reduced scenario's workspace.
        let original_signature = Signature::of(&original, &scenario.initial);
        let ops_before = scenario.ops.len();

        let reduction = reduce_with(&runner, scenario, 0, 1000, || EmptyReference);

        assert_eq!(
            Signature::of(&reduction.finding, &reduction.scenario.initial),
            original_signature
        );
        assert!(reduction.scenario.ops.len() < ops_before);
    }

    /// Removing an earlier file renumbers a later one's `FileId` without
    /// changing which file it is, so the signature must follow the logical
    /// path rather than the index.
    #[test]
    fn test_signature_preserves_logical_identity_across_an_earlier_file_removal() {
        let before = workspace(&["a.R", "b.R"]);
        let after_removal = workspace(&["b.R"]);

        let before_signature = Signature::of(&mismatch(1, "val_0", true), &before);
        let after_signature = Signature::of(&mismatch(0, "val_0", true), &after_removal);

        assert_eq!(before_signature, after_signature);
    }

    /// `catch_quietly()` is not reentrant. `reduce()` catches candidate panics
    /// through `compare()`, so this test catches its own panic directly.
    #[test]
    fn test_reduce_rejects_a_scenario_that_never_mismatches_under_fresh() {
        let scenario = corpus::scenario("rename_and_undo_across_files");
        let runner = Runner::open();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            reduce(&runner, scenario, 0, 10)
        }));

        let payload = match result {
            Err(payload) => payload,
            Ok(_) => panic!("expected reduce() to reject a scenario that does not mismatch"),
        };
        let message = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("<non-string panic payload>");
        assert!(message.contains("does not reproduce"));
    }

    /// A new instance is created for each candidate so its behavior cannot
    /// depend on state from an earlier candidate.
    enum Flaky {
        Empty,
        Panics,
        Recovers,
    }

    impl Reference for Flaky {
        fn observe(
            &mut self,
            _spec: &WorkspaceSpec,
            _query: &Query,
        ) -> (Observation, Option<Range<usize>>) {
            match self {
                Flaky::Empty => (Observation { resolved: vec![] }, None),
                Flaky::Panics => panic!("flaky reference: injected panic"),
                Flaky::Recovers => (Observation { resolved: vec![] }, Some(0..1)),
            }
        }
    }

    /// Exercises deterministic panic and recovery patterns while shrinking.
    #[test]
    fn test_reduce_survives_interleaved_panics_and_skips() {
        let scenario = corpus::scenario("rename_and_undo_across_files");
        let runner = Runner::open();
        let original = match compare(&runner, &scenario, EmptyReference) {
            Outcome::Mismatch(finding) => finding,
            other => panic!("expected a mismatch, got {other:?}"),
        };
        let original_signature = Signature::of(&original, &scenario.initial);

        let mut calls = 0usize;
        let reduction = reduce_with(&runner, scenario, 0, 1000, move || {
            calls += 1;
            match calls {
                // Establish the mismatch that later candidates must preserve.
                1 => Flaky::Empty,
                n if n % 5 == 0 => Flaky::Panics,
                n if n % 7 == 0 => Flaky::Recovers,
                _ => Flaky::Empty,
            }
        });

        assert_eq!(
            Signature::of(&reduction.finding, &reduction.scenario.initial),
            original_signature
        );
        assert!(!reduction.shrink_panics.is_empty());
    }
}
