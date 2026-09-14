//! Semantic mutations over a [`Scenario`].
//!
//! A derived mutator would waste checks on numeric digits and invalid
//! identifiers. These mutations instead change source edges, call resolution,
//! statement nesting, files, and edit history.
//!
//! Growth thresholds are checked while selecting candidates because `Check`
//! repeatedly mutates its generated corpus. Candidate filters also avoid common
//! no-op mutations.

use mutatis::Candidates;
use mutatis::Mutate;
use mutatis::Result;
use oak_semantic::effects::fuzz::callee;
use oak_semantic::effects::fuzz::EffectRecipe;
use oak_semantic::effects::fuzz::Form;
use oak_semantic::effects::fuzz::SourceProvider;
use oak_semantic::effects::TargetAccess;
use oak_semantic::fuzz::Block;
use oak_semantic::fuzz::Expr;
use oak_semantic::fuzz::Invocation;
use oak_semantic::fuzz::Program;
use oak_semantic::fuzz::Stmt;
use oak_semantic::semantic_index::EvalEnv;
use oak_semantic::semantic_index::EvalTiming;

use crate::fuzz::build::binding;
use crate::fuzz::build::function_def;
use crate::fuzz::build::library;
use crate::fuzz::build::shadow;
use crate::fuzz::build::source_with;
use crate::fuzz::choose::binding_name;
use crate::fuzz::choose::random_query;
use crate::fuzz::choose::Choose;
use crate::fuzz::generate::file_path;
use crate::fuzz::generate::UNINSTALLED;
use crate::fuzz::scenario::Edit;
use crate::fuzz::scenario::Op;
use crate::fuzz::scenario::Scenario;
use crate::fuzz::spec::FileId;
use crate::fuzz::spec::FileSpec;

pub(super) const MAX_FILES: usize = 5;

/// Stop selecting programs at this nested-statement count. One compound
/// insertion may cross the threshold.
const MAX_STATEMENTS: usize = 14;

/// Allow a top-level statement plus two nested statement levels.
const MAX_DEPTH: usize = 3;

/// Fit the longest [`seed_corpus()`] history, three rounds of five operations.
///
/// [`seed_corpus()`]: crate::fuzz::generate::seed_corpus
const MAX_OPS: usize = 16;

/// Stop growing a program past this rendered width, which the statement count
/// cannot detect. The statement that crosses the line still lands.
const MAX_TEXT: usize = 2_000;

pub struct ScenarioMutator;

impl Mutate<Scenario> for ScenarioMutator {
    fn mutate(&mut self, mutations: &mut Candidates<'_>, scenario: &mut Scenario) -> Result<()> {
        let shrink = mutations.shrink();

        for step in STEPS {
            if shrink && !step.shrinks() {
                continue;
            }
            if !step.applies(scenario) {
                continue;
            }
            mutations.mutation(|context| {
                step.apply(context.rng(), scenario);
                Ok(())
            })?;
        }

        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
enum Step {
    AddSourceEdge,
    RedirectSourceEdge,
    RemoveSourceEdge,
    SwapProvider,
    FlipInvocation,
    InsertStatement,
    RemoveStatement,
    ReorderStatements,
    ShadowCallee,
    NestStatement,
    UnnestStatement,
    AddFile,
    RemoveFile,
    ChangeColdEntry,
    InsertOp,
    AlterOp,
    RemoveOp,
}

const STEPS: [Step; 17] = [
    Step::AddSourceEdge,
    Step::RedirectSourceEdge,
    Step::RemoveSourceEdge,
    Step::SwapProvider,
    Step::FlipInvocation,
    Step::InsertStatement,
    Step::RemoveStatement,
    Step::ReorderStatements,
    Step::ShadowCallee,
    Step::NestStatement,
    Step::UnnestStatement,
    Step::AddFile,
    Step::RemoveFile,
    Step::ChangeColdEntry,
    Step::InsertOp,
    Step::AlterOp,
    Step::RemoveOp,
];

impl Step {
    /// Shrinking registers only steps that make the scenario smaller or
    /// shallower.
    fn shrinks(self) -> bool {
        matches!(
            self,
            Step::RemoveSourceEdge |
                Step::RemoveStatement |
                Step::UnnestStatement |
                Step::RemoveFile |
                Step::RemoveOp
        )
    }

    /// Read without mutation because candidate registration must be
    /// deterministic. Shrinking stops when every predicate is false.
    fn applies(self, scenario: &Scenario) -> bool {
        match self {
            Step::AddSourceEdge | Step::InsertStatement => {
                !insertable_block_slots(scenario).is_empty()
            },
            Step::ShadowCallee => !shadowable_block_slots(scenario).is_empty(),
            Step::RedirectSourceEdge | Step::RemoveSourceEdge | Step::SwapProvider => {
                !slots_where(scenario, is_source).is_empty()
            },
            Step::FlipInvocation => !slots_where(scenario, is_flippable).is_empty(),
            Step::RemoveStatement => !slots_where(scenario, |_| true).is_empty(),
            Step::ReorderStatements => !reorderable_block_slots(scenario).is_empty(),
            Step::NestStatement => !nestable_slots(scenario).is_empty(),
            Step::UnnestStatement => slots_where(scenario, |_| true)
                .iter()
                .any(|slot| slot.path.len() >= 2),
            Step::AddFile => scenario.initial.files.len() < MAX_FILES,
            Step::RemoveFile => scenario.initial.files.len() > 1,
            Step::ChangeColdEntry => true,
            Step::InsertOp => scenario.ops.len() < MAX_OPS,
            Step::AlterOp => !alterable_ops(scenario).is_empty(),
            Step::RemoveOp => !scenario.ops.is_empty(),
        }
    }

    fn apply(self, rng: &mut impl Choose, scenario: &mut Scenario) {
        match self {
            Step::AddSourceEdge => add_source_edge(rng, scenario),
            Step::RedirectSourceEdge => redirect_source_edge(rng, scenario),
            Step::RemoveSourceEdge => remove_slot(rng, scenario, slots_where(scenario, is_source)),
            Step::SwapProvider => swap_provider(rng, scenario),
            Step::FlipInvocation => flip_invocation(rng, scenario),
            Step::InsertStatement => insert_statement(rng, scenario),
            Step::RemoveStatement => remove_slot(rng, scenario, slots_where(scenario, |_| true)),
            Step::ReorderStatements => reorder_statements(rng, scenario),
            Step::ShadowCallee => shadow_callee(rng, scenario),
            Step::NestStatement => nest_statement(rng, scenario),
            Step::UnnestStatement => unnest_statement(rng, scenario),
            Step::AddFile => add_file(scenario),
            Step::RemoveFile => remove_file(rng, scenario),
            Step::ChangeColdEntry => {
                scenario.cold_entry = random_query(rng, scenario.initial.files.len())
            },
            Step::InsertOp => insert_op(rng, scenario),
            Step::AlterOp => alter_op(rng, scenario),
            Step::RemoveOp => {
                let index = rng.index(scenario.ops.len());
                scenario.ops.remove(index);
            },
        }
    }
}

// == Source edges ==

fn add_source_edge(rng: &mut impl Choose, scenario: &mut Scenario) {
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

fn redirect_source_edge(rng: &mut impl Choose, scenario: &mut Scenario) {
    let targets = source_targets(scenario);
    let Some(slot) = pick(rng, slots_where(scenario, is_source)) else {
        return;
    };
    let Some(Stmt::Effect {
        recipe: EffectRecipe::Source { path, .. },
        ..
    }) = statement_mut(scenario, &slot)
    else {
        return;
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

fn swap_provider(rng: &mut impl Choose, scenario: &mut Scenario) {
    const PROVIDERS: [SourceProvider; 3] = [
        SourceProvider::File,
        SourceProvider::Dir,
        SourceProvider::FileOrDir,
    ];

    let Some(slot) = pick(rng, slots_where(scenario, is_source)) else {
        return;
    };
    let Some(Stmt::Effect {
        recipe: EffectRecipe::Source { provider, .. },
        ..
    }) = statement_mut(scenario, &slot)
    else {
        return;
    };
    let others: Vec<SourceProvider> = PROVIDERS
        .into_iter()
        .filter(|candidate| candidate != provider)
        .collect();
    *provider = others[rng.index(others.len())];
}

fn flip_invocation(rng: &mut impl Choose, scenario: &mut Scenario) {
    let Some(slot) = pick(rng, slots_where(scenario, is_flippable)) else {
        return;
    };
    let Some(Stmt::Effect { invocation, .. }) = statement_mut(scenario, &slot) else {
        return;
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
    if scenario.initial.package.is_some() {
        targets.push("R".to_string());
    }
    targets
}

// == Statements ==

fn insert_statement(rng: &mut impl Choose, scenario: &mut Scenario) {
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

fn shadow_callee(rng: &mut impl Choose, scenario: &mut Scenario) {
    let Some(slot) = pick(rng, shadowable_block_slots(scenario)) else {
        return;
    };
    let names: Vec<&'static str> = match programs(scenario).get(slot.program) {
        Some(program) => program.callees().iter().map(|callee| callee.name).collect(),
        None => return,
    };
    let Some(name) = pick(rng, names) else {
        return;
    };
    insert_at(rng, scenario, &slot, shadow(name));
}

fn reorder_statements(rng: &mut impl Choose, scenario: &mut Scenario) {
    let Some(slot) = pick(rng, reorderable_block_slots(scenario)) else {
        return;
    };
    let Some(block) = block_mut(scenario, &slot) else {
        return;
    };
    if block.len() < 2 {
        return;
    }
    let first = rng.index(block.len());
    let second = (first + 1 + rng.index(block.len() - 1)) % block.len();
    block.swap(first, second);
}

/// Move bodyless statements into nested blocks to exercise context-sensitive
/// effect handling.
fn nest_statement(rng: &mut impl Choose, scenario: &mut Scenario) {
    let Some(moved) = pick(rng, nestable_slots(scenario)) else {
        return;
    };
    let Some(mut target) = pick(rng, nest_targets(scenario, &moved)) else {
        return;
    };

    let Some(program) = program_mut(scenario, moved.program) else {
        return;
    };
    let Some(stmt) = take_statement(&mut program.statements, &moved.path) else {
        return;
    };
    rebase_after_removal(&mut target.path, &moved.path);
    match block_at_mut(&mut program.statements, &target.path) {
        Some(block) => {
            let index = rng.index(block.len() + 1);
            block.insert(index, stmt);
        },
        None => program.statements.push(stmt),
    }
}

/// Require another block in the same program. A program with only top-level
/// statements offers no destination outside the statement's parent.
fn nestable_slots(scenario: &Scenario) -> Vec<Slot> {
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

fn unnest_statement(rng: &mut impl Choose, scenario: &mut Scenario) {
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
        return;
    };
    let Some(stmt) = take_statement(&mut program.statements, &moved.path) else {
        return;
    };
    match block_at_mut(&mut program.statements, &target) {
        Some(block) => block.insert(index.min(block.len()), stmt),
        None => program.statements.push(stmt),
    }
}

fn remove_slot(rng: &mut impl Choose, scenario: &mut Scenario, slots: Vec<Slot>) {
    let Some(slot) = pick(rng, slots) else {
        return;
    };
    let Some(program) = program_mut(scenario, slot.program) else {
        return;
    };
    let Some((owner, index)) = owner_of(&mut program.statements, &slot.path) else {
        return;
    };
    owner.remove(index);
}

/// Callers use [`insertable_block_slots()`] to enforce growth thresholds. The
/// inserted statement's own body determines whether it fits the depth limit.
fn insert_at(rng: &mut impl Choose, scenario: &mut Scenario, slot: &Slot, stmt: Stmt) {
    if slot.path.len() + height(&stmt) > MAX_DEPTH {
        return;
    }
    let Some(program) = program_mut(scenario, slot.program) else {
        return;
    };
    let Some(block) = block_at_mut(&mut program.statements, &slot.path) else {
        return;
    };
    let index = rng.index(block.len() + 1);
    block.insert(index, stmt);
}

// == Files ==

fn add_file(scenario: &mut Scenario) {
    if scenario.initial.files.len() >= MAX_FILES {
        return;
    }
    let Some(owner) = scenario.initial.files.first().map(|file| file.owner) else {
        return;
    };
    let Some(index) = (0..MAX_FILES).find(|&index| {
        let path = file_path(owner, index);
        !scenario.initial.files.iter().any(|file| file.path == path)
    }) else {
        return;
    };

    scenario.initial.files.push(FileSpec {
        owner,
        path: file_path(owner, index),
        program: Program {
            statements: vec![binding(&binding_name(index))],
        },
    });
}

fn remove_file(rng: &mut impl Choose, scenario: &mut Scenario) {
    if scenario.initial.files.len() <= 1 {
        return;
    }
    let removed = rng.index(scenario.initial.files.len());
    scenario.initial.files.remove(removed);
    repair_file_ids(scenario, removed);
}

/// Retarget operations to live files. Leave stale `source()` paths intact so
/// the runner still exercises unresolved files.
fn repair_file_ids(scenario: &mut Scenario, removed: usize) {
    let count = scenario.initial.files.len();
    if count == 0 {
        return;
    }
    if let Some(file) = scenario.cold_entry.file_mut() {
        rebase_file(file, removed, count);
    }
    for op in &mut scenario.ops {
        if let Some(file) = op.file_mut() {
            rebase_file(file, removed, count);
        }
    }
}

fn rebase_file(file: &mut FileId, removed: usize, count: usize) {
    let index = match file.0 {
        index if index < removed => index,
        index if index > removed => index - 1,
        _ => removed,
    };
    file.0 = index.min(count - 1);
}

// == History ==

fn insert_op(rng: &mut impl Choose, scenario: &mut Scenario) {
    if scenario.ops.len() >= MAX_OPS {
        return;
    }
    let files = scenario.initial.files.len();
    let op = if rng.odds(60) {
        Op::Query(random_query(rng, files))
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

fn alter_op(rng: &mut impl Choose, scenario: &mut Scenario) {
    let files = scenario.initial.files.len();
    let Some(index) = pick(rng, alterable_ops(scenario)) else {
        return;
    };
    match &mut scenario.ops[index] {
        Op::Query(query) => *query = random_query(rng, files),
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
fn alterable_ops(scenario: &Scenario) -> Vec<usize> {
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

// == Scenario traversal ==

/// Address a statement by its program and index path through nested blocks.
#[derive(Clone, Debug)]
struct Slot {
    program: usize,
    path: Vec<usize>,
}

/// Include programs from the initial workspace and every edit replacement.
fn programs(scenario: &Scenario) -> Vec<&Program> {
    let mut out: Vec<&Program> = scenario
        .initial
        .files
        .iter()
        .map(|file| &file.program)
        .collect();
    out.extend(scenario.ops.iter().filter_map(|op| match op {
        Op::Edit(edit) => Some(&edit.program),
        Op::Query(_) => None,
    }));
    out
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

fn program_mut(scenario: &mut Scenario, index: usize) -> Option<&mut Program> {
    programs_mut(scenario).into_iter().nth(index)
}

fn statement_mut<'scenario>(
    scenario: &'scenario mut Scenario,
    slot: &Slot,
) -> Option<&'scenario mut Stmt> {
    let program = program_mut(scenario, slot.program)?;
    let (owner, index) = owner_of(&mut program.statements, &slot.path)?;
    owner.get_mut(index)
}

fn block_mut<'scenario>(
    scenario: &'scenario mut Scenario,
    slot: &Slot,
) -> Option<&'scenario mut Block> {
    let program = program_mut(scenario, slot.program)?;
    block_at_mut(&mut program.statements, &slot.path)
}

fn slots_where(scenario: &Scenario, keep: impl Fn(&Stmt) -> bool) -> Vec<Slot> {
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
fn block_slots(scenario: &Scenario) -> Vec<Slot> {
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

fn open_block_slots(scenario: &Scenario) -> Vec<Slot> {
    let mut slots = block_slots(scenario);
    slots.retain(|slot| slot.path.len() < MAX_DEPTH);
    slots
}

/// Exclude programs that have reached a statement or text threshold before
/// registering an insertion.
fn insertable_block_slots(scenario: &Scenario) -> Vec<Slot> {
    let programs = programs(scenario);
    let mut slots = open_block_slots(scenario);
    slots.retain(|slot| match programs.get(slot.program) {
        Some(program) => has_room(program),
        None => false,
    });
    slots
}

/// Require an existing callee so an inserted shadow can affect resolution.
fn shadowable_block_slots(scenario: &Scenario) -> Vec<Slot> {
    let programs = programs(scenario);
    let mut slots = insertable_block_slots(scenario);
    slots.retain(|slot| match programs.get(slot.program) {
        Some(program) => !program.callees().is_empty(),
        None => false,
    });
    slots
}

fn reorderable_block_slots(scenario: &Scenario) -> Vec<Slot> {
    let programs = programs(scenario);
    let mut slots = block_slots(scenario);
    slots.retain(|slot| match programs.get(slot.program) {
        Some(program) => match block_at(&program.statements, &slot.path) {
            Some(block) => block.len() >= 2,
            None => false,
        },
        None => false,
    });
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

fn block_at<'block>(block: &'block Block, path: &[usize]) -> Option<&'block Block> {
    let mut current = block;
    for &index in path {
        current = child_block(current.get(index)?)?;
    }
    Some(current)
}

fn block_at_mut<'block>(block: &'block mut Block, path: &[usize]) -> Option<&'block mut Block> {
    let mut current = block;
    for &index in path {
        current = child_block_mut(current.get_mut(index)?)?;
    }
    Some(current)
}

fn owner_of<'block>(
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

fn take_statement(block: &mut Block, path: &[usize]) -> Option<Stmt> {
    let (owner, index) = owner_of(block, path)?;
    Some(owner.remove(index))
}

/// Rebases a block address after `removed` was taken out from under it.
fn rebase_after_removal(path: &mut [usize], removed: &[usize]) {
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

fn child_block(stmt: &Stmt) -> Option<&Block> {
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
fn height(stmt: &Stmt) -> usize {
    match child_block(stmt) {
        Some(body) => 1 + body.iter().map(height).max().unwrap_or(0),
        None => 1,
    }
}

fn count_statements(block: &Block) -> usize {
    block
        .iter()
        .map(|stmt| 1 + child_block(stmt).map_or(0, count_statements))
        .sum()
}

fn has_room(program: &Program) -> bool {
    count_statements(&program.statements) < MAX_STATEMENTS && program.render().text.len() < MAX_TEXT
}

fn is_bodyless(stmt: &Stmt) -> bool {
    child_block(stmt).is_none()
}

fn is_source(stmt: &Stmt) -> bool {
    matches!(stmt, Stmt::Effect {
        recipe: EffectRecipe::Source { .. },
        ..
    })
}

/// Qualifying renders `pkg::name()`, so it changes nothing without a package or
/// with infix syntax. Matches `qualifies()` in `oak_semantic`.
fn is_flippable(stmt: &Stmt) -> bool {
    let Stmt::Effect { recipe, .. } = stmt else {
        return false;
    };
    let target = callee(recipe);
    target.package.is_some() && target.form == Form::Call
}

// == Choices ==

fn pick<T>(rng: &mut impl Choose, items: Vec<T>) -> Option<T> {
    if items.is_empty() {
        return None;
    }
    let index = rng.index(items.len());
    items.into_iter().nth(index)
}

fn random_provider(rng: &mut impl Choose) -> SourceProvider {
    match rng.index(3) {
        0 => SourceProvider::File,
        1 => SourceProvider::Dir,
        _ => SourceProvider::FileOrDir,
    }
}

fn random_invocation(rng: &mut impl Choose) -> Invocation {
    if rng.odds(50) {
        Invocation::Bare
    } else {
        Invocation::Qualified
    }
}

fn attachable(rng: &mut impl Choose, scenario: &Scenario) -> String {
    let candidates: Vec<&String> = scenario
        .initial
        .installed
        .iter()
        .filter(|name| *name != "base")
        .collect();
    if candidates.is_empty() || rng.odds(15) {
        return UNINSTALLED.to_string();
    }
    candidates[rng.index(candidates.len())].clone()
}

#[cfg(test)]
mod tests {
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    use super::*;
    use crate::fuzz::corpus;

    /// Test source-edge steps directly because `InsertStatement` can also add
    /// `source()` calls.
    #[test]
    fn test_source_edge_steps_add_and_remove_an_edge() {
        let mut rng = StdRng::seed_from_u64(0);
        let mut scenario = corpus::case("acyclic_pair_closes_then_reopens");
        let edges = slots_where(&scenario, is_source).len();

        Step::AddSourceEdge.apply(&mut rng, &mut scenario);
        assert_eq!(slots_where(&scenario, is_source).len(), edges + 1);

        Step::RemoveSourceEdge.apply(&mut rng, &mut scenario);
        assert_eq!(slots_where(&scenario, is_source).len(), edges);
    }
}
