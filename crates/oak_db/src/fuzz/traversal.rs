//! Addresses programs and nested blocks shared by validation and mutation.

use oak_semantic::effects::fuzz::EffectRecipe;
use oak_semantic::fuzz::Block;
use oak_semantic::fuzz::Expr;
use oak_semantic::fuzz::Program;
use oak_semantic::fuzz::Stmt;

use crate::fuzz::scenario::Op;
use crate::fuzz::scenario::Scenario;
use crate::fuzz::spec::FileId;
use crate::fuzz::spec::Owner;

// == Scenario traversal ==

/// Address a statement by its program and index path through nested blocks.
#[derive(Clone, Debug)]
pub(super) struct Slot {
    pub(super) program: usize,
    pub(super) path: Vec<usize>,
}

/// Include programs from the initial workspace and every edit replacement.
pub(super) fn programs(scenario: &Scenario) -> Vec<&Program> {
    sited_programs(scenario)
        .into_iter()
        .map(|(_, program)| program)
        .collect()
}

/// Match the file and operation labels in the artifact when reporting a
/// rejected program.
pub(super) enum ProgramSite {
    File(usize),
    Op(usize),
}

impl ProgramSite {
    pub(super) fn render(&self) -> String {
        match self {
            ProgramSite::File(index) => format!("file [{index}]"),
            ProgramSite::Op(index) => format!("op {index}"),
        }
    }
}

/// Share the traversal between validation and mutation so both include edit
/// replacements in the same order.
pub(super) fn sited_programs(scenario: &Scenario) -> Vec<(ProgramSite, &Program)> {
    let mut out: Vec<(ProgramSite, &Program)> = scenario
        .initial
        .files
        .iter()
        .enumerate()
        .map(|(index, file)| (ProgramSite::File(index), &file.program))
        .collect();
    out.extend(
        scenario
            .ops
            .iter()
            .enumerate()
            .filter_map(|(index, op)| match op {
                Op::Edit(edit) => Some((ProgramSite::Op(index), &edit.program)),
                Op::Query(_) => None,
            }),
    );
    out
}

/// Returns the owner whose root resolves relative paths in `slot`. An edit
/// replacement retains the owner of the file it replaces.
pub(super) fn slot_owner(scenario: &Scenario, slot: &Slot) -> Owner {
    let file = match sited_programs(scenario)[slot.program].0 {
        ProgramSite::File(index) => FileId(index),
        ProgramSite::Op(index) => match &scenario.ops[index] {
            Op::Edit(edit) => edit.file,
            Op::Query(_) => panic!("slot {slot:?} addresses a query"),
        },
    };
    scenario.initial.file(file).owner
}

fn programs_mut(scenario: &mut Scenario) -> Vec<&mut Program> {
    let mut out: Vec<&mut Program> = scenario
        .initial
        .files
        .iter_mut()
        .map(|file| &mut file.program)
        .collect();
    out.extend(scenario.ops.iter_mut().filter_map(|op| match op {
        Op::Edit(edit) => Some(&mut edit.program),
        Op::Query(_) => None,
    }));
    out
}

pub(super) fn program_mut(scenario: &mut Scenario, index: usize) -> Option<&mut Program> {
    programs_mut(scenario).into_iter().nth(index)
}

pub(super) fn statement_mut<'scenario>(
    scenario: &'scenario mut Scenario,
    slot: &Slot,
) -> Option<&'scenario mut Stmt> {
    let program = program_mut(scenario, slot.program)?;
    let (owner, index) = owner_of(&mut program.statements, &slot.path)?;
    owner.get_mut(index)
}

pub(super) fn block_mut<'scenario>(
    scenario: &'scenario mut Scenario,
    slot: &Slot,
) -> Option<&'scenario mut Block> {
    let program = program_mut(scenario, slot.program)?;
    block_at_mut(&mut program.statements, &slot.path)
}

pub(super) fn slots_where(scenario: &Scenario, keep: impl Fn(&Stmt) -> bool) -> Vec<Slot> {
    let mut slots = Vec::new();
    for (index, program) in programs(scenario).into_iter().enumerate() {
        let mut path = Vec::new();
        collect_statement_slots(&program.statements, index, &mut path, &keep, &mut slots);
    }
    slots
}

fn collect_statement_slots(
    block: &Block,
    program: usize,
    path: &mut Vec<usize>,
    keep: &impl Fn(&Stmt) -> bool,
    slots: &mut Vec<Slot>,
) {
    for (index, stmt) in block.iter().enumerate() {
        path.push(index);
        if keep(stmt) {
            slots.push(Slot {
                program,
                path: path.clone(),
            });
        }
        if let Some(body) = child_block(stmt) {
            collect_statement_slots(body, program, path, keep, slots);
        }
        path.pop();
    }
}

/// Every block, addressed by the path of the statement that owns it. The empty
/// path is a program's top level.
pub(super) fn block_slots(scenario: &Scenario) -> Vec<Slot> {
    let mut slots = Vec::new();
    for (index, program) in programs(scenario).into_iter().enumerate() {
        slots.push(Slot {
            program: index,
            path: Vec::new(),
        });
        let mut path = Vec::new();
        collect_block_slots(&program.statements, index, &mut path, &mut slots);
    }
    slots
}

fn collect_block_slots(
    block: &Block,
    program: usize,
    path: &mut Vec<usize>,
    slots: &mut Vec<Slot>,
) {
    for (index, stmt) in block.iter().enumerate() {
        let Some(body) = child_block(stmt) else {
            continue;
        };
        path.push(index);
        slots.push(Slot {
            program,
            path: path.clone(),
        });
        collect_block_slots(body, program, path, slots);
        path.pop();
    }
}

// == Block navigation ==

pub(super) fn block_at<'block>(block: &'block Block, path: &[usize]) -> Option<&'block Block> {
    let mut current = block;
    for &index in path {
        current = child_block(current.get(index)?)?;
    }
    Some(current)
}

pub(super) fn block_at_mut<'block>(
    block: &'block mut Block,
    path: &[usize],
) -> Option<&'block mut Block> {
    let mut current = block;
    for &index in path {
        current = child_block_mut(current.get_mut(index)?)?;
    }
    Some(current)
}

pub(super) fn owner_of<'block>(
    block: &'block mut Block,
    path: &[usize],
) -> Option<(&'block mut Block, usize)> {
    let (last, parent) = path.split_last()?;
    let owner = block_at_mut(block, parent)?;
    if *last >= owner.len() {
        return None;
    }
    Some((owner, *last))
}

pub(super) fn take_statement(block: &mut Block, path: &[usize]) -> Option<Stmt> {
    let (owner, index) = owner_of(block, path)?;
    Some(owner.remove(index))
}

/// Rebases a block address after `removed` was taken out from under it.
pub(super) fn rebase_after_removal(path: &mut [usize], removed: &[usize]) {
    let Some((last, parent)) = removed.split_last() else {
        return;
    };
    if path.len() <= parent.len() || path[..parent.len()] != *parent {
        return;
    }
    if path[parent.len()] > *last {
        path[parent.len()] -= 1;
    }
}

pub(super) fn child_block(stmt: &Stmt) -> Option<&Block> {
    match stmt {
        Stmt::Bind { value, .. } | Stmt::Expr(value) => expr_block(value),
        Stmt::Effect { recipe, .. } => recipe_block(recipe),
    }
}

fn expr_block(expr: &Expr) -> Option<&Block> {
    match expr {
        Expr::Function { body } | Expr::Hole(body) => Some(body),
        Expr::Num(_) | Expr::Null | Expr::Ident(_) | Expr::Call { .. } => None,
    }
}

fn recipe_block(recipe: &EffectRecipe) -> Option<&Block> {
    match recipe {
        EffectRecipe::Eval { body, .. } |
        EffectRecipe::Quote { body } |
        EffectRecipe::QuoteHoles { body } |
        EffectRecipe::Substitute { body } => Some(body),
        EffectRecipe::Assign { value, .. } | EffectRecipe::Rebind { value, .. } => {
            expr_block(value)
        },
        EffectRecipe::Source { .. } | EffectRecipe::Attach { .. } => None,
    }
}

fn child_block_mut(stmt: &mut Stmt) -> Option<&mut Block> {
    match stmt {
        Stmt::Bind { value, .. } | Stmt::Expr(value) => expr_block_mut(value),
        Stmt::Effect { recipe, .. } => recipe_block_mut(recipe),
    }
}

fn expr_block_mut(expr: &mut Expr) -> Option<&mut Block> {
    match expr {
        Expr::Function { body } | Expr::Hole(body) => Some(body),
        Expr::Num(_) | Expr::Null | Expr::Ident(_) | Expr::Call { .. } => None,
    }
}

fn recipe_block_mut(recipe: &mut EffectRecipe) -> Option<&mut Block> {
    match recipe {
        EffectRecipe::Eval { body, .. } |
        EffectRecipe::Quote { body } |
        EffectRecipe::QuoteHoles { body } |
        EffectRecipe::Substitute { body } => Some(body),
        EffectRecipe::Assign { value, .. } | EffectRecipe::Rebind { value, .. } => {
            expr_block_mut(value)
        },
        EffectRecipe::Source { .. } | EffectRecipe::Attach { .. } => None,
    }
}

/// Count occupied statement levels. Empty bodies add no depth.
pub(super) fn height(stmt: &Stmt) -> usize {
    match child_block(stmt) {
        Some(body) => 1 + body.iter().map(height).max().unwrap_or(0),
        None => 1,
    }
}

pub(super) fn count_statements(block: &Block) -> usize {
    block
        .iter()
        .map(|stmt| 1 + child_block(stmt).map_or(0, count_statements))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rebase_changes_only_later_siblings_and_their_descendants() {
        let cases = [
            (vec![2, 3], vec![1], vec![1, 3]),
            (vec![0, 2, 3], vec![0, 1], vec![0, 1, 3]),
            (vec![0, 0], vec![0, 1], vec![0, 0]),
            (vec![1, 2], vec![0, 1], vec![1, 2]),
            (vec![0], vec![0, 1], vec![0]),
            (vec![], vec![0], vec![]),
        ];
        for (mut path, removed, expected) in cases {
            rebase_after_removal(&mut path, &removed);
            assert_eq!(path, expected);
        }
    }
}
