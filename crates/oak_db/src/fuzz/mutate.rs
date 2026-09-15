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
use oak_semantic::fuzz::Expr;
use oak_semantic::fuzz::Invocation;
use oak_semantic::fuzz::Program;
use oak_semantic::fuzz::Stmt;
use oak_semantic::semantic_index::EvalEnv;
use oak_semantic::semantic_index::EvalTiming;

use crate::fuzz::budgets::MAX_DEPTH;
use crate::fuzz::budgets::MAX_EXPORTS;
use crate::fuzz::budgets::MAX_FILES;
use crate::fuzz::budgets::MAX_OPS;
use crate::fuzz::budgets::MAX_PACKAGES;
use crate::fuzz::budgets::MAX_REEXPORTS;
use crate::fuzz::budgets::MAX_STATEMENTS;
use crate::fuzz::budgets::MAX_TEXT;
use crate::fuzz::build::binding;
use crate::fuzz::build::function_def;
use crate::fuzz::build::library;
use crate::fuzz::build::shadow;
use crate::fuzz::build::source_with;
use crate::fuzz::choose::binding_name;
use crate::fuzz::choose::export_name;
use crate::fuzz::choose::random_query;
use crate::fuzz::choose::Choose;
use crate::fuzz::choose::Shape;
use crate::fuzz::choose::EXPORT_NAMES;
use crate::fuzz::generate::file_path;
use crate::fuzz::generate::UNINSTALLED;
use crate::fuzz::scenario::Edit;
use crate::fuzz::scenario::Op;
use crate::fuzz::scenario::Query;
use crate::fuzz::scenario::Scenario;
use crate::fuzz::spec::FileId;
use crate::fuzz::spec::FileSpec;
use crate::fuzz::spec::Owner;
use crate::fuzz::spec::PackageId;
use crate::fuzz::spec::PackageKind;
use crate::fuzz::spec::PackageSpec;
use crate::fuzz::spec::Reexport;
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

// Favor queries so edit histories exercise several entry points per edit.
const QUERY_PERCENT: u32 = 60;
// Keep unresolved attachments reachable without dominating live packages.
const UNINSTALLED_PERCENT: u32 = 15;

// == Choice vocabularies ==

const PROVIDERS: [SourceProvider; 3] = [
    SourceProvider::File,
    SourceProvider::Dir,
    SourceProvider::FileOrDir,
];
const PACKAGE_NAMES: [&str; 3] = ["lib0", "lib1", "lib2"];

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
    AddReexportEdge,
    RedirectReexportEdge,
    RemoveReexportEdge,
    AddExport,
    RemoveExport,
    AddPackage,
    RemovePackage,
}

const STEPS: [Step; 24] = [
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
    Step::AddReexportEdge,
    Step::RedirectReexportEdge,
    Step::RemoveReexportEdge,
    Step::AddExport,
    Step::RemoveExport,
    Step::AddPackage,
    Step::RemovePackage,
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
                Step::RemoveOp |
                Step::RemoveReexportEdge |
                Step::RemoveExport |
                Step::RemovePackage
        )
    }

    /// Read without mutation because candidate registration must be
    /// deterministic. Shrinking stops when every predicate is false.
    fn applies(self, scenario: &Scenario) -> bool {
        match self {
            Step::AddSourceEdge | Step::InsertStatement => {
                !insertable_block_slots(scenario).is_empty()
            },
            Step::ShadowCallee => !shadowable_slots(scenario).is_empty(),
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
            Step::AddReexportEdge => !packages_with_reexport_room(scenario).is_empty(),
            Step::RedirectReexportEdge | Step::RemoveReexportEdge => {
                !reexport_slots(scenario).is_empty()
            },
            Step::AddExport => !packages_with_export_room(scenario).is_empty(),
            Step::RemoveExport => !packages_with_exports(scenario).is_empty(),
            Step::AddPackage => {
                scenario.initial.packages.len() < MAX_PACKAGES &&
                    !available_package_names(scenario).is_empty()
            },
            Step::RemovePackage => !removable_packages(scenario).is_empty(),
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
            Step::AddFile => add_file(rng, scenario),
            Step::RemoveFile => remove_file(rng, scenario),
            Step::ChangeColdEntry => {
                scenario.cold_entry =
                    different_query(rng, &Shape::of(&scenario.initial), &scenario.cold_entry)
            },
            Step::InsertOp => insert_op(rng, scenario),
            Step::AlterOp => alter_op(rng, scenario),
            Step::RemoveOp => {
                let index = rng.index(scenario.ops.len());
                scenario.ops.remove(index);
            },
            Step::AddReexportEdge => add_reexport_edge(rng, scenario),
            Step::RedirectReexportEdge => redirect_reexport_edge(rng, scenario),
            Step::RemoveReexportEdge => remove_reexport_edge(rng, scenario),
            Step::AddExport => add_export(rng, scenario),
            Step::RemoveExport => remove_export(rng, scenario),
            Step::AddPackage => add_package(rng, scenario),
            Step::RemovePackage => remove_package(rng, scenario),
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

fn swap_provider(rng: &mut impl Choose, scenario: &mut Scenario) {
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

fn flip_invocation(rng: &mut impl Choose, scenario: &mut Scenario) {
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

fn reorder_statements(rng: &mut impl Choose, scenario: &mut Scenario) {
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
fn nest_statement(rng: &mut impl Choose, scenario: &mut Scenario) {
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

fn remove_slot(rng: &mut impl Choose, scenario: &mut Scenario, slots: Vec<Slot>) {
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

// == Files ==

fn add_file(rng: &mut impl Choose, scenario: &mut Scenario) {
    if scenario.initial.files.len() >= MAX_FILES {
        return;
    }
    let mut owners = vec![Owner::Script];
    owners.extend(
        scenario
            .initial
            .packages
            .iter()
            .enumerate()
            .filter_map(|(index, package)| {
                (package.kind == PackageKind::Workspace).then_some(Owner::Package(PackageId(index)))
            }),
    );
    let owner = owners[rng.index(owners.len())];
    let Some(index) = (0..MAX_FILES).find(|&index| {
        let path = file_path(owner, index);
        // Different packages can each own `R/a.R` under their own roots.
        !scenario
            .initial
            .files
            .iter()
            .any(|file| file.owner == owner && file.path == path)
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

fn alter_op(rng: &mut impl Choose, scenario: &mut Scenario) {
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

// == Packages ==

/// Combines `choose::export_name()`'s `exp_*` vocabulary with `val_*` binding
/// names, so a mutated export can either match another package's reexport or
/// a name a file actually binds.
fn export_vocabulary() -> Vec<String> {
    (0..EXPORT_NAMES)
        .map(export_name)
        .chain((0..MAX_FILES).map(binding_name))
        .collect()
}

fn add_export(rng: &mut impl Choose, scenario: &mut Scenario) {
    let Some(index) = pick(rng, packages_with_export_room(scenario)) else {
        return;
    };
    let exports = &mut scenario.initial.packages[index].exports;
    let free = export_vocabulary()
        .into_iter()
        .filter(|name| !exports.contains(name))
        .collect();
    let Some(export) = pick(rng, free) else {
        panic!("export candidate has no available names");
    };
    exports.push(export);
}

fn remove_export(rng: &mut impl Choose, scenario: &mut Scenario) {
    let Some(index) = pick(rng, packages_with_exports(scenario)) else {
        return;
    };
    let exports = &mut scenario.initial.packages[index].exports;
    let removed = rng.index(exports.len());
    exports.remove(removed);
}

fn packages_with_export_room(scenario: &Scenario) -> Vec<usize> {
    (0..scenario.initial.packages.len())
        .filter(|&index| {
            let exports = &scenario.initial.packages[index].exports;
            exports.len() < MAX_EXPORTS &&
                export_vocabulary()
                    .iter()
                    .any(|name| !exports.contains(name))
        })
        .collect()
}

fn packages_with_exports(scenario: &Scenario) -> Vec<usize> {
    (0..scenario.initial.packages.len())
        .filter(|&index| !scenario.initial.packages[index].exports.is_empty())
        .collect()
}

fn add_reexport_edge(rng: &mut impl Choose, scenario: &mut Scenario) {
    let Some(index) = pick(rng, packages_with_reexport_room(scenario)) else {
        return;
    };
    let from = reexport_source(rng, scenario);
    // Keep one `importFrom()` per name so parsing cannot discard an edge.
    let taken: Vec<&str> = scenario.initial.packages[index]
        .reexports
        .iter()
        .map(|reexport| reexport.name.as_str())
        .collect();
    let free: Vec<String> = (0..EXPORT_NAMES)
        .map(export_name)
        .filter(|name| !taken.contains(&name.as_str()))
        .collect();
    let Some(name) = pick(rng, free) else {
        return;
    };
    scenario.initial.packages[index]
        .reexports
        .push(Reexport { name, from });
}

fn redirect_reexport_edge(rng: &mut impl Choose, scenario: &mut Scenario) {
    let Some((package, reexport)) = pick(rng, reexport_slots(scenario)) else {
        return;
    };
    let current = scenario.initial.packages[package].reexports[reexport]
        .from
        .clone();
    let others: Vec<String> = reexport_sources(scenario)
        .into_iter()
        .filter(|candidate| *candidate != current)
        .collect();
    let Some(from) = pick(rng, others) else {
        return;
    };
    scenario.initial.packages[package].reexports[reexport].from = from;
}

fn remove_reexport_edge(rng: &mut impl Choose, scenario: &mut Scenario) {
    let Some((package, reexport)) = pick(rng, reexport_slots(scenario)) else {
        return;
    };
    scenario.initial.packages[package]
        .reexports
        .remove(reexport);
}

fn packages_with_reexport_room(scenario: &Scenario) -> Vec<usize> {
    (0..scenario.initial.packages.len())
        .filter(|&index| {
            let reexports = &scenario.initial.packages[index].reexports;
            reexports.len() < MAX_REEXPORTS &&
                (0..EXPORT_NAMES)
                    .map(export_name)
                    .any(|name| !reexports.iter().any(|edge| edge.name == name))
        })
        .collect()
}

fn reexport_slots(scenario: &Scenario) -> Vec<(usize, usize)> {
    scenario
        .initial
        .packages
        .iter()
        .enumerate()
        .flat_map(|(package, spec)| {
            (0..spec.reexports.len()).map(move |reexport| (package, reexport))
        })
        .collect()
}

fn reexport_source(rng: &mut impl Choose, scenario: &Scenario) -> String {
    let sources = reexport_sources(scenario);
    sources[rng.index(sources.len())].clone()
}

/// Modeled package names plus one known-absent name, so a dangling import
/// stays reachable.
fn reexport_sources(scenario: &Scenario) -> Vec<String> {
    let mut sources: Vec<String> = scenario
        .initial
        .packages
        .iter()
        .map(|package| package.name.clone())
        .collect();
    sources.push(UNINSTALLED.to_string());
    sources
}

fn add_package(rng: &mut impl Choose, scenario: &mut Scenario) {
    if scenario.initial.packages.len() >= MAX_PACKAGES {
        return;
    }
    let Some(name) = pick(rng, available_package_names(scenario)) else {
        return;
    };
    scenario.initial.packages.push(PackageSpec {
        name: name.to_string(),
        kind: if rng.odds(50) {
            PackageKind::Library
        } else {
            PackageKind::Workspace
        },
        exports: Vec::new(),
        reexports: Vec::new(),
    });
}

fn available_package_names(scenario: &Scenario) -> Vec<&'static str> {
    PACKAGE_NAMES
        .into_iter()
        .filter(|name| {
            !scenario
                .initial
                .packages
                .iter()
                .any(|package| package.name == *name) &&
                !scenario
                    .initial
                    .installed
                    .iter()
                    .any(|installed| installed == name)
        })
        .collect()
}

/// Delete owned files with the package. Moving them to another package would
/// change the root used to resolve their `source()` paths.
fn remove_package(rng: &mut impl Choose, scenario: &mut Scenario) {
    let Some(removed) = pick(rng, removable_packages(scenario)) else {
        return;
    };
    remove_owned_files(scenario, removed);
    scenario.initial.packages.remove(removed);
    repair_package_ids(rng, scenario, removed);
}

/// Packages whose removal still leaves at least one file, matching the
/// `validate()` rule that a workspace has files.
fn removable_packages(scenario: &Scenario) -> Vec<usize> {
    let total = scenario.initial.files.len();
    (0..scenario.initial.packages.len())
        .filter(|&index| owned_file_count(scenario, index) < total)
        .collect()
}

fn owned_file_count(scenario: &Scenario, package: usize) -> usize {
    scenario
        .initial
        .files
        .iter()
        .filter(|file| matches!(file.owner, Owner::Package(id) if id.0 == package))
        .count()
}

/// Remove higher file indices first so pending removals keep their indices.
fn remove_owned_files(scenario: &mut Scenario, package: usize) {
    let owned: Vec<usize> = scenario
        .initial
        .files
        .iter()
        .enumerate()
        .filter(|(_, file)| matches!(file.owner, Owner::Package(id) if id.0 == package))
        .map(|(index, _)| index)
        .collect();
    for index in owned.into_iter().rev() {
        scenario.initial.files.remove(index);
        repair_file_ids(scenario, index);
    }
}

/// Retarget queries to surviving packages. If none remain, replace package
/// queries with queries supported by the remaining workspace.
fn repair_package_ids(rng: &mut impl Choose, scenario: &mut Scenario, removed: usize) {
    let count = scenario.initial.packages.len();
    if count == 0 {
        let shape = Shape::of(&scenario.initial);
        if scenario.cold_entry.package().is_some() {
            scenario.cold_entry = random_query(rng, &shape);
        }
        for op in &mut scenario.ops {
            if let Op::Query(query) = op {
                if query.package().is_some() {
                    *query = random_query(rng, &shape);
                }
            }
        }
        return;
    }

    if let Some(id) = scenario.cold_entry.package_mut() {
        rebase_package(id, removed, count);
    }
    for op in &mut scenario.ops {
        if let Op::Query(query) = op {
            if let Some(id) = query.package_mut() {
                rebase_package(id, removed, count);
            }
        }
    }
    for file in &mut scenario.initial.files {
        if let Owner::Package(id) = &mut file.owner {
            rebase_package(id, removed, count);
        }
    }
}

fn rebase_package(id: &mut PackageId, removed: usize, count: usize) {
    let index = match id.0 {
        index if index < removed => index,
        index if index > removed => index - 1,
        _ => removed,
    };
    id.0 = index.min(count - 1);
}

// == Candidate selection ==

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
    slots.retain(|slot| has_room(programs[slot.program]));
    slots
}

/// Select bare calls whose own block can receive a shadow binding.
fn shadowable_slots(scenario: &Scenario) -> Vec<Slot> {
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

fn reorderable_block_slots(scenario: &Scenario) -> Vec<Slot> {
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
    PROVIDERS[rng.index(PROVIDERS.len())]
}

/// Keep the existing distribution for the first draw. A collision switches to
/// a different aggregate without an unbounded rejection loop.
fn different_query(rng: &mut impl Choose, shape: &Shape, current: &Query) -> Query {
    let candidate = random_query(rng, shape);
    if candidate != *current {
        return candidate;
    }
    match current {
        Query::AllPackageDependencies => Query::AllWorkspaceFileDependencies,
        _ => Query::AllPackageDependencies,
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

#[cfg(test)]
mod tests {
    use mutatis::Session;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    use super::*;
    use crate::fuzz::build::qualified_source;
    use crate::fuzz::build::source;
    use crate::fuzz::corpus;
    use crate::fuzz::limits::within_bounds;
    use crate::fuzz::seed_corpus;

    struct FixedChoice(usize);

    impl Choose for FixedChoice {
        fn index(&mut self, len: usize) -> usize {
            assert!(len > 0);
            self.0 % len
        }

        fn odds(&mut self, _percent: u32) -> bool {
            false
        }
    }

    fn scenario_with(statements: Vec<Stmt>) -> Scenario {
        let mut scenario = corpus::case("acyclic_pair_closes_then_reopens");
        scenario.initial.files.truncate(1);
        scenario.initial.files[0].program = Program { statements };
        scenario.ops.clear();
        scenario.cold_entry = Query::Diagnostics(FileId(0));
        scenario
    }

    #[test]
    fn test_nesting_rebases_a_later_destination() {
        let mut scenario = scenario_with(vec![binding("value"), function_def("fun", vec![])]);
        nest_statement(&mut FixedChoice(0), &mut scenario);
        let expected = Program {
            statements: vec![function_def("fun", vec![binding("value")])],
        };
        assert_eq!(
            scenario.initial.files[0].program.render().text,
            expected.render().text
        );

        unnest_statement(&mut FixedChoice(0), &mut scenario);
        let expected = Program {
            statements: vec![function_def("fun", vec![]), binding("value")],
        };
        assert_eq!(
            scenario.initial.files[0].program.render().text,
            expected.render().text
        );
    }

    #[test]
    fn test_shadow_is_inserted_before_a_bare_call_in_its_own_block() {
        let mut scenario = scenario_with(vec![
            qualified_source("a.R"),
            function_def("fun", vec![source("a.R")]),
        ]);
        shadow_callee(&mut FixedChoice(0), &mut scenario);
        let expected = Program {
            statements: vec![
                qualified_source("a.R"),
                function_def("fun", vec![shadow("source"), source("a.R")]),
            ],
        };
        assert_eq!(
            scenario.initial.files[0].program.render().text,
            expected.render().text
        );
        let qualified = scenario_with(vec![qualified_source("a.R")]);
        assert!(!Step::ShadowCallee.applies(&qualified));
    }

    #[test]
    fn test_query_collision_still_changes_the_query() {
        let scenario = scenario_with(vec![]);
        let shape = Shape::of(&scenario.initial);
        let current = random_query(&mut FixedChoice(0), &shape);
        assert_eq!(
            different_query(&mut FixedChoice(0), &shape, &current),
            Query::AllPackageDependencies
        );
    }

    #[test]
    fn test_exports_do_not_repeat_existing_names() {
        let mut scenario = scenario_with(vec![]);
        add_package(&mut FixedChoice(0), &mut scenario);
        for _ in 0..MAX_EXPORTS {
            add_export(&mut FixedChoice(0), &mut scenario);
        }
        assert_eq!(scenario.initial.packages[0].exports, [
            "exp_0", "exp_1", "exp_2"
        ]);
        assert!(!Step::AddExport.applies(&scenario));
    }

    #[test]
    fn test_package_and_file_mutations_rebuild_workspace_ownership() {
        let mut scenario = scenario_with(vec![]);
        add_package(&mut FixedChoice(0), &mut scenario);
        assert_eq!(scenario.initial.packages[0].kind, PackageKind::Workspace);
        add_file(&mut FixedChoice(1), &mut scenario);
        assert_eq!(
            scenario.initial.files[1].owner,
            Owner::Package(PackageId(0))
        );
        assert_eq!(scenario.initial.files[1].path, "R/a.R");
        assert!(scenario.validate().is_ok());

        remove_package(&mut FixedChoice(0), &mut scenario);
        assert_eq!(scenario.initial.files.len(), 1);
        assert_eq!(scenario.initial.packages.len(), 0);
        assert!(scenario.validate().is_ok());

        add_package(&mut FixedChoice(0), &mut scenario);
        add_file(&mut FixedChoice(1), &mut scenario);
        add_file(&mut FixedChoice(0), &mut scenario);
        assert_eq!(
            scenario.initial.files[1].owner,
            Owner::Package(PackageId(0))
        );
        assert_eq!(scenario.initial.files[2].owner, Owner::Script);
        assert!(scenario.validate().is_ok());
    }

    #[test]
    fn test_every_shrink_step_makes_progress() {
        let mut rng = StdRng::seed_from_u64(0);
        let mut corpus = seed_corpus(0);
        corpus.push(scenario_with(vec![function_def("outer", vec![
            function_def("inner", vec![binding("value")]),
        ])]));
        for step in STEPS.into_iter().filter(|step| step.shrinks()) {
            let mut exercised = false;
            for original in &corpus {
                if !step.applies(original) {
                    continue;
                }
                let mut scenario = original.clone();
                let before = complexity(&scenario);
                step.apply(&mut rng, &mut scenario);
                assert!(complexity(&scenario) < before);
                assert!(scenario.validate().is_ok());
                exercised = true;
            }
            assert!(exercised);
        }
    }

    // Unnesting keeps node counts unchanged but decreases total statement depth.
    fn complexity(scenario: &Scenario) -> (usize, usize) {
        let slots = slots_where(scenario, |_| true);
        let count = scenario.initial.files.len() +
            scenario.initial.packages.len() +
            scenario.ops.len() +
            slots.len() +
            scenario
                .initial
                .packages
                .iter()
                .map(|package| package.exports.len() + package.reexports.len())
                .sum::<usize>();
        (count, slots.iter().map(|slot| slot.path.len()).sum())
    }

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

    /// Check that repeated mutations keep initial and replacement programs
    /// within the limits imposed on decoded scenarios.
    #[test]
    fn test_mutation_respects_declared_bounds() {
        let mut session = Session::new().seed(0);
        let mut corpus = seed_corpus(0);

        for round in 0..20_000 {
            let entry = round % corpus.len();
            if session
                .mutate_with(&mut ScenarioMutator, &mut corpus[entry])
                .is_err()
            {
                continue;
            }

            let scenario = &corpus[entry];
            assert!(scenario.initial.files.len() <= MAX_FILES);
            assert!(scenario.ops.len() <= MAX_OPS);
            assert!(scenario.initial.packages.len() <= MAX_PACKAGES);
            for package in &scenario.initial.packages {
                assert!(package.exports.len() <= MAX_EXPORTS);
                assert!(package.reexports.len() <= MAX_REEXPORTS);
            }
            for program in programs(scenario) {
                assert!(count_statements(&program.statements) <= MAX_STATEMENTS + 1);

                for stmt in &program.statements {
                    assert!(height(stmt) <= MAX_DEPTH);
                }
            }

            assert!(within_bounds(scenario).is_ok());
        }
    }
}
