//! Mutates edit histories and selects replacement queries.

use super::pick;
use crate::fuzz::budgets::MAX_OPS;
use crate::fuzz::choose::observed_file;
use crate::fuzz::choose::observing_query;
use crate::fuzz::choose::random_query;
use crate::fuzz::choose::Choose;
use crate::fuzz::choose::Shape;
use crate::fuzz::scenario::Edit;
use crate::fuzz::scenario::Op;
use crate::fuzz::scenario::Query;
use crate::fuzz::scenario::Scenario;
use crate::fuzz::spec::FileId;

// == Sampling choices ==

// Favor queries so edit histories exercise several entry points per edit.
const QUERY_PERCENT: u32 = 60;

/// Probability of choosing a direct observer instead of the full query
/// distribution.
const OBSERVE_EDIT_PERCENT: u32 = 70;

// == History ==

pub(super) fn insert_op(rng: &mut impl Choose, scenario: &mut Scenario) {
    if scenario.ops.len() >= MAX_OPS {
        return;
    }
    let shape = Shape::of(&scenario.initial);

    if rng.odds(QUERY_PERCENT) {
        // Append so the observer follows the selected replacement regardless of
        // where that edit appears.
        let pending = pick(rng, pending_replacements(scenario, scenario.ops.len()));
        if let Some(file) = pending.filter(|_| rng.odds(OBSERVE_EDIT_PERCENT)) {
            scenario
                .ops
                .push(Op::Query(observing_query(rng, &shape, file)));
            return;
        }
        let index = rng.index(scenario.ops.len() + 1);
        scenario
            .ops
            .insert(index, Op::Query(random_query(rng, &shape)));
        return;
    }

    let index = rng.index(scenario.ops.len() + 1);
    let file = FileId(rng.index(scenario.initial.files.len()));
    scenario.ops.insert(
        index,
        Op::Edit(Edit {
            file,
            program: scenario.initial.file(file).program.clone(),
        }),
    );

    // Pair edits with observing queries without making them inseparable during shrinking.
    if scenario.ops.len() < MAX_OPS && rng.odds(OBSERVE_EDIT_PERCENT) {
        scenario
            .ops
            .insert(index + 1, Op::Query(observing_query(rng, &shape, file)));
    }
}

/// Returns files whose latest replacement in `ops[..until]` has no later direct
/// observer recognized by [`observed_file()`]. Aggregate and indirect queries
/// may still analyze a pending file. Recomputing from the current history repairs
/// pairings removed or retargeted by later mutations.
pub(super) fn pending_replacements(scenario: &Scenario, until: usize) -> Vec<FileId> {
    pending_with(&scenario.ops[..until], None)
}

/// Simulates one query retarget so [`settleable_queries()`] can reject changes
/// that do not reduce the final pending set.
fn pending_with(ops: &[Op], observer: Option<(usize, FileId)>) -> Vec<FileId> {
    let mut pending: Vec<FileId> = Vec::new();
    for (index, op) in ops.iter().enumerate() {
        let substituted = match observer {
            Some((at, file)) if at == index => Some(file),
            _ => None,
        };
        if let Some(file) = substituted {
            pending.retain(|candidate| *candidate != file);
            continue;
        }
        match op {
            Op::Edit(edit) => {
                if !pending.contains(&edit.file) {
                    pending.push(edit.file);
                }
            },
            Op::Query(query) => {
                if let Some(observed) = observed_file(query) {
                    pending.retain(|candidate| *candidate != observed);
                }
            },
        }
    }
    pending
}

pub(super) fn alter_op(rng: &mut impl Choose, scenario: &mut Scenario) {
    let files = scenario.initial.files.len();
    let shape = Shape::of(&scenario.initial);

    // At `MAX_OPS`, only retargeting can add a direct observer. Prefer a repair,
    // while leaving uniform alteration free to remove observations.
    let settleable = if rng.odds(OBSERVE_EDIT_PERCENT) {
        pick(rng, settleable_queries(scenario))
    } else {
        None
    };
    if let Some((index, file)) = settleable {
        let Op::Query(query) = &mut scenario.ops[index] else {
            panic!("settleable operation {index} is not a query");
        };
        *query = observing_query(rng, &shape, file);
        return;
    }

    let Some(index) = pick(rng, alterable_ops(scenario)) else {
        return;
    };
    match &mut scenario.ops[index] {
        Op::Query(query) => *query = different_query(rng, &shape, query),
        Op::Edit(edit) => {
            let others: Vec<usize> = (0..files).filter(|&file| file != edit.file.0).collect();
            let Some(file) = pick(rng, others) else {
                return;
            };
            edit.file = FileId(file);
        },
    }
}

/// Returns query/file substitutions that reduce the pending set after the whole
/// history. Whole-history comparison rejects exchanging observers and queries
/// before a later replacement of the same file. A final replacement with no
/// later query cannot be repaired by retargeting.
pub(super) fn settleable_queries(scenario: &Scenario) -> Vec<(usize, FileId)> {
    let pending = pending_replacements(scenario, scenario.ops.len());
    if pending.is_empty() {
        return Vec::new();
    }

    let mut settleable = Vec::new();
    for (index, op) in scenario.ops.iter().enumerate() {
        if !matches!(op, Op::Query(_)) {
            continue;
        }
        for &file in &pending {
            if pending_with(&scenario.ops, Some((index, file))).len() < pending.len() {
                settleable.push((index, file));
            }
        }
    }
    settleable
}

/// Operations that have another value to take. A lone file leaves an `Edit`
/// nowhere to retarget.
pub(super) fn alterable_ops(scenario: &Scenario) -> Vec<usize> {
    let files = scenario.initial.files.len();
    scenario
        .ops
        .iter()
        .enumerate()
        .filter(|(_, op)| match op {
            Op::Query(_) => true,
            Op::Edit(_) => files > 1,
        })
        .map(|(index, _)| index)
        .collect()
}

/// Keep the existing distribution for the first draw. A collision switches to
/// a different aggregate without an unbounded rejection loop.
pub(super) fn different_query(rng: &mut impl Choose, shape: &Shape, current: &Query) -> Query {
    let candidate = random_query(rng, shape);
    if candidate != *current {
        return candidate;
    }
    match current {
        Query::AllPackageDependencies => Query::AllWorkspaceFileDependencies,
        _ => Query::AllPackageDependencies,
    }
}
