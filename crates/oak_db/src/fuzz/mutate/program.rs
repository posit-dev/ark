//! Mutates source calls, statements, and their nesting.

use oak_semantic::effects::fuzz::callee;
use oak_semantic::effects::fuzz::EffectRecipe;
use oak_semantic::effects::fuzz::Form;
use oak_semantic::effects::fuzz::SourceProvider;
use oak_semantic::effects::TargetAccess;
use oak_semantic::fuzz::Expr;
use oak_semantic::fuzz::Invocation;
use oak_semantic::fuzz::Program;
use oak_semantic::fuzz::Stmt;
use oak_semantic::semantic_index::EvalEnv;
use oak_semantic::semantic_index::EvalTiming;

use super::pick;
use crate::fuzz::budgets::MAX_DEPTH;
use crate::fuzz::budgets::MAX_FILES;
use crate::fuzz::budgets::MAX_STATEMENTS;
use crate::fuzz::budgets::MAX_TEXT;
use crate::fuzz::build::binding;
use crate::fuzz::build::function_def;
use crate::fuzz::build::library;
use crate::fuzz::build::shadow;
use crate::fuzz::build::source_with;
use crate::fuzz::choose::binding_name;
use crate::fuzz::choose::Choose;
use crate::fuzz::generate::UNINSTALLED;
use crate::fuzz::scenario::Scenario;
use crate::fuzz::spec::PackageKind;
use crate::fuzz::traversal::block_at;
use crate::fuzz::traversal::block_at_mut;
use crate::fuzz::traversal::block_mut;
use crate::fuzz::traversal::block_slots;
use crate::fuzz::traversal::child_block;
use crate::fuzz::traversal::count_statements;
use crate::fuzz::traversal::height;
use crate::fuzz::traversal::owner_of;
use crate::fuzz::traversal::program_mut;
use crate::fuzz::traversal::programs;
use crate::fuzz::traversal::rebase_after_removal;
use crate::fuzz::traversal::slots_where;
use crate::fuzz::traversal::statement_mut;
use crate::fuzz::traversal::take_statement;
use crate::fuzz::traversal::Slot;

// == Sampling choices ==

// Keep unresolved attachments reachable without dominating live packages.
const UNINSTALLED_PERCENT: u32 = 15;

// == Choice vocabularies ==

const PROVIDERS: [SourceProvider; 3] = [
    SourceProvider::File,
    SourceProvider::Dir,
    SourceProvider::FileOrDir,
];

// == Source edges ==

pub(super) fn add_source_edge(rng: &mut impl Choose, scenario: &mut Scenario) {
    let target = random_target(rng, scenario);
    let provider = random_provider(rng);
    let invocation = random_invocation(rng);
    let Some(slot) = pick(rng, insertable_block_slots(scenario)) else {
        return;
    };
    insert_at(
        rng,
        scenario,
        &slot,
        source_with(&target, provider, invocation),
    );
}

pub(super) fn redirect_source_edge(rng: &mut impl Choose, scenario: &mut Scenario) {
    let targets = source_targets(scenario);
    let Some(slot) = pick(rng, slots_where(scenario, is_source)) else {
        return;
    };
    let Some(Stmt::Effect {
        recipe: EffectRecipe::Source { path, .. },
        ..
    }) = statement_mut(scenario, &slot)
    else {
        panic!("source candidate is not a source: {slot:?}");
    };
    let others: Vec<String> = targets
        .into_iter()
        .filter(|candidate| candidate != path)
        .collect();
    let Some(target) = pick(rng, others) else {
        return;
    };
    *path = target;
}

pub(super) fn swap_provider(rng: &mut impl Choose, scenario: &mut Scenario) {
    let Some(slot) = pick(rng, slots_where(scenario, is_source)) else {
        return;
    };
    let Some(Stmt::Effect {
        recipe: EffectRecipe::Source { provider, .. },
        ..
    }) = statement_mut(scenario, &slot)
    else {
        panic!("source candidate is not a source: {slot:?}");
    };
    let others: Vec<SourceProvider> = PROVIDERS
        .into_iter()
        .filter(|candidate| candidate != provider)
        .collect();
    *provider = others[rng.index(others.len())];
}

pub(super) fn flip_invocation(rng: &mut impl Choose, scenario: &mut Scenario) {
    let Some(slot) = pick(rng, slots_where(scenario, is_flippable)) else {
        return;
    };
    let Some(Stmt::Effect { invocation, .. }) = statement_mut(scenario, &slot) else {
        panic!("invocation candidate is not an effect: {slot:?}");
    };
    *invocation = match invocation {
        Invocation::Bare => Invocation::Qualified,
        Invocation::Qualified => Invocation::Bare,
    };
}

fn random_target(rng: &mut impl Choose, scenario: &Scenario) -> String {
    let targets = source_targets(scenario);
    targets[rng.index(targets.len())].clone()
}

/// Cover every [`SourceProvider`] with a live file, a directory, or a package's
/// `R/` directory. The `.` entry keeps the result nonempty.
fn source_targets(scenario: &Scenario) -> Vec<String> {
    let mut targets: Vec<String> = scenario
        .initial
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect();
    targets.push(".".to_string());
    if scenario
        .initial
        .packages
        .iter()
        .any(|package| package.kind == PackageKind::Workspace)
    {
        targets.push("R".to_string());
    }
    targets
}

// == Statements ==

pub(super) fn insert_statement(rng: &mut impl Choose, scenario: &mut Scenario) {
    let Some(slot) = pick(rng, insertable_block_slots(scenario)) else {
        return;
    };
    let stmt = palette_statement(rng, scenario, MAX_DEPTH - slot.path.len());
    insert_at(rng, scenario, &slot, stmt);
}

/// Cover every renderable recipe, not only the seed corpus's `source()` and
/// `library()` calls. `room` limits the inserted statement's nesting depth.
fn palette_statement(rng: &mut impl Choose, scenario: &Scenario, room: usize) -> Stmt {
    let name = binding_name(rng.index(MAX_FILES));
    match rng.index(11) {
        0 => binding(&name),
        1 => function_def(&name, vec![]),
        2 => Stmt::use_of(&name),
        3 => library(&attachable(rng, scenario)),
        4 => source_with(
            &random_target(rng, scenario),
            random_provider(rng),
            random_invocation(rng),
        ),
        5 => Stmt::effect(
            EffectRecipe::Assign {
                name,
                value: Expr::Num(1),
            },
            random_invocation(rng),
        ),
        6 => Stmt::effect(
            EffectRecipe::Rebind {
                name,
                value: Expr::Call {
                    name: "identity".to_string(),
                },
                target: if rng.odds(50) {
                    TargetAccess::Write
                } else {
                    TargetAccess::ReadWrite
                },
            },
            Invocation::Bare,
        ),
        7 => Stmt::effect(
            EffectRecipe::Eval {
                env: if rng.odds(50) {
                    EvalEnv::Current
                } else {
                    EvalEnv::Nested
                },
                timing: if rng.odds(50) {
                    EvalTiming::Eager
                } else {
                    EvalTiming::Lazy
                },
                body: vec![],
            },
            random_invocation(rng),
        ),
        8 => Stmt::effect(EffectRecipe::Quote { body: vec![] }, random_invocation(rng)),
        // The empty hole lets nesting build `bquote(.(source("b.R")))`.
        // Omit it when the destination has no remaining depth.
        9 => Stmt::effect(
            EffectRecipe::QuoteHoles {
                body: match room {
                    0 | 1 => vec![],
                    _ => vec![Stmt::Expr(Expr::Hole(vec![]))],
                },
            },
            random_invocation(rng),
        ),
        _ => Stmt::effect(
            EffectRecipe::Substitute { body: vec![] },
            random_invocation(rng),
        ),
    }
}

pub(super) fn shadow_callee(rng: &mut impl Choose, scenario: &mut Scenario) {
    let Some(slot) = pick(rng, shadowable_slots(scenario)) else {
        return;
    };
    let Some(Stmt::Effect { recipe, .. }) = statement_mut(scenario, &slot) else {
        panic!("shadow candidate is not an effect: {slot:?}");
    };
    let name = callee(recipe).name;
    let Some(program) = program_mut(scenario, slot.program) else {
        panic!("invalid shadow program: {slot:?}");
    };
    let Some((block, index)) = owner_of(&mut program.statements, &slot.path) else {
        panic!("invalid shadow slot: {slot:?}");
    };
    // Put the binding in the callee's own block, immediately before the call.
    block.insert(index, shadow(name));
}

pub(super) fn reorder_statements(rng: &mut impl Choose, scenario: &mut Scenario) {
    let Some(slot) = pick(rng, reorderable_block_slots(scenario)) else {
        return;
    };
    let Some(block) = block_mut(scenario, &slot) else {
        panic!("selected mutation address is invalid");
    };
    assert!(block.len() >= 2);
    let first = rng.index(block.len());
    let second = (first + 1 + rng.index(block.len() - 1)) % block.len();
    block.swap(first, second);
}

/// Move bodyless statements into nested blocks to exercise context-sensitive
/// effect handling.
pub(super) fn nest_statement(rng: &mut impl Choose, scenario: &mut Scenario) {
    let Some(moved) = pick(rng, nestable_slots(scenario)) else {
        return;
    };
    let Some(mut target) = pick(rng, nest_targets(scenario, &moved)) else {
        panic!("nesting candidate has no destination: {moved:?}");
    };

    let Some(program) = program_mut(scenario, moved.program) else {
        panic!("selected mutation address is invalid");
    };
    let Some(stmt) = take_statement(&mut program.statements, &moved.path) else {
        panic!("selected mutation address is invalid");
    };
    rebase_after_removal(&mut target.path, &moved.path);
    match block_at_mut(&mut program.statements, &target.path) {
        Some(block) => {
            let index = rng.index(block.len() + 1);
            block.insert(index, stmt);
        },
        None => panic!("destination became invalid after moving {moved:?}"),
    }
}

/// Require another block in the same program. A program with only top-level
/// statements offers no destination outside the statement's parent.
pub(super) fn nestable_slots(scenario: &Scenario) -> Vec<Slot> {
    let mut slots = slots_where(scenario, is_bodyless);
    slots.retain(|slot| !nest_targets(scenario, slot).is_empty());
    slots
}

fn nest_targets(scenario: &Scenario, moved: &Slot) -> Vec<Slot> {
    let parent = &moved.path[..moved.path.len() - 1];
    open_block_slots(scenario)
        .into_iter()
        .filter(|slot| slot.program == moved.program && slot.path != parent)
        .collect()
}

pub(super) fn unnest_statement(rng: &mut impl Choose, scenario: &mut Scenario) {
    let deep: Vec<Slot> = slots_where(scenario, |_| true)
        .into_iter()
        .filter(|slot| slot.path.len() >= 2)
        .collect();
    let Some(moved) = pick(rng, deep) else {
        return;
    };

    let depth = moved.path.len();
    let target = moved.path[..depth - 2].to_vec();
    let index = moved.path[depth - 2] + 1;

    let Some(program) = program_mut(scenario, moved.program) else {
        panic!("selected mutation address is invalid");
    };
    let Some(stmt) = take_statement(&mut program.statements, &moved.path) else {
        panic!("selected mutation address is invalid");
    };
    match block_at_mut(&mut program.statements, &target) {
        Some(block) => block.insert(index, stmt),
        None => panic!("destination became invalid after moving {moved:?}"),
    }
}

pub(super) fn remove_slot(rng: &mut impl Choose, scenario: &mut Scenario, slots: Vec<Slot>) {
    let Some(slot) = pick(rng, slots) else {
        return;
    };
    let Some(program) = program_mut(scenario, slot.program) else {
        panic!("selected mutation address is invalid");
    };
    let Some((owner, index)) = owner_of(&mut program.statements, &slot.path) else {
        panic!("selected mutation address is invalid");
    };
    owner.remove(index);
}

/// Callers use [`insertable_block_slots()`] to enforce growth thresholds. The
/// inserted statement's own body determines whether it fits the depth limit.
fn insert_at(rng: &mut impl Choose, scenario: &mut Scenario, slot: &Slot, stmt: Stmt) {
    assert!(slot.path.len() + height(&stmt) <= MAX_DEPTH);
    let Some(program) = program_mut(scenario, slot.program) else {
        panic!("selected mutation address is invalid");
    };
    let Some(block) = block_at_mut(&mut program.statements, &slot.path) else {
        panic!("selected mutation address is invalid");
    };
    let index = rng.index(block.len() + 1);
    block.insert(index, stmt);
}

// == Candidate selection ==

fn open_block_slots(scenario: &Scenario) -> Vec<Slot> {
    let mut slots = block_slots(scenario);
    slots.retain(|slot| slot.path.len() < MAX_DEPTH);
    slots
}

/// Exclude programs that have reached a statement or text threshold before
/// registering an insertion.
pub(super) fn insertable_block_slots(scenario: &Scenario) -> Vec<Slot> {
    let programs = programs(scenario);
    let mut slots = open_block_slots(scenario);
    slots.retain(|slot| has_room(programs[slot.program]));
    slots
}

/// Select bare calls whose own block can receive a shadow binding.
pub(super) fn shadowable_slots(scenario: &Scenario) -> Vec<Slot> {
    let programs = programs(scenario);
    slots_where(scenario, |stmt| {
        matches!(stmt, Stmt::Effect {
            invocation: Invocation::Bare,
            ..
        })
    })
    .into_iter()
    .filter(|slot| has_room(programs[slot.program]))
    .collect()
}

pub(super) fn reorderable_block_slots(scenario: &Scenario) -> Vec<Slot> {
    let programs = programs(scenario);
    let mut slots = block_slots(scenario);
    slots.retain(
        |slot| match block_at(&programs[slot.program].statements, &slot.path) {
            Some(block) => block.len() >= 2,
            None => panic!("invalid block candidate: {slot:?}"),
        },
    );
    slots
}

fn has_room(program: &Program) -> bool {
    count_statements(&program.statements) < MAX_STATEMENTS && program.render().text.len() < MAX_TEXT
}

fn is_bodyless(stmt: &Stmt) -> bool {
    child_block(stmt).is_none()
}

pub(super) fn is_source(stmt: &Stmt) -> bool {
    matches!(stmt, Stmt::Effect {
        recipe: EffectRecipe::Source { .. },
        ..
    })
}

/// Qualifying renders `pkg::name()`, so it changes nothing without a package or
/// with infix syntax. Matches `qualifies()` in `oak_semantic`.
pub(super) fn is_flippable(stmt: &Stmt) -> bool {
    let Stmt::Effect { recipe, .. } = stmt else {
        return false;
    };
    let target = callee(recipe);
    target.package.is_some() && target.form == Form::Call
}

fn random_provider(rng: &mut impl Choose) -> SourceProvider {
    PROVIDERS[rng.index(PROVIDERS.len())]
}

fn random_invocation(rng: &mut impl Choose) -> Invocation {
    if rng.odds(50) {
        Invocation::Bare
    } else {
        Invocation::Qualified
    }
}

fn attachable(rng: &mut impl Choose, scenario: &Scenario) -> String {
    let candidates: Vec<&str> = scenario
        .initial
        .installed
        .iter()
        .map(String::as_str)
        .chain(
            scenario
                .initial
                .packages
                .iter()
                .map(|package| package.name.as_str()),
        )
        .filter(|name| *name != "base")
        .collect();
    if candidates.is_empty() || rng.odds(UNINSTALLED_PERCENT) {
        return UNINSTALLED.to_string();
    }
    candidates[rng.index(candidates.len())].to_string()
}
