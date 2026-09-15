//! Mutates edit histories and selects replacement queries.

use super::pick;
use crate::fuzz::budgets::MAX_OPS;
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

// == History ==

pub(super) fn insert_op(rng: &mut impl Choose, scenario: &mut Scenario) {
    if scenario.ops.len() >= MAX_OPS {
        return;
    }
    let files = scenario.initial.files.len();
    let op = if rng.odds(QUERY_PERCENT) {
        Op::Query(random_query(rng, &Shape::of(&scenario.initial)))
    } else {
        let file = FileId(rng.index(files));
        Op::Edit(Edit {
            file,
            program: scenario.initial.file(file).program.clone(),
        })
    };
    let index = rng.index(scenario.ops.len() + 1);
    scenario.ops.insert(index, op);
}

pub(super) fn alter_op(rng: &mut impl Choose, scenario: &mut Scenario) {
    let files = scenario.initial.files.len();
    let Some(index) = pick(rng, alterable_ops(scenario)) else {
        return;
    };
    match &mut scenario.ops[index] {
        Op::Query(query) => *query = different_query(rng, &Shape::of(&scenario.initial), query),
        Op::Edit(edit) => {
            let others: Vec<usize> = (0..files).filter(|&file| file != edit.file.0).collect();
            let Some(file) = pick(rng, others) else {
                return;
            };
            edit.file = FileId(file);
        },
    }
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
