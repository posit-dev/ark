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
use crate::fuzz::spec::is_identifier;
use crate::fuzz::spec::FileId;
use crate::fuzz::spec::FileSpec;
use crate::fuzz::spec::Owner;
use crate::fuzz::spec::PackageId;
use crate::fuzz::spec::PackageKind;
use crate::fuzz::spec::PackageSpec;
use crate::fuzz::spec::Reexport;

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

/// Allow decoded programs to exceed the mutation threshold by a bounded margin.
/// A compound insertion can cross [`MAX_STATEMENTS`] in one step.
const CEILING_STATEMENTS: usize = 2 * MAX_STATEMENTS;

/// Limit rendered bytes while allowing an insertion to overshoot [`MAX_TEXT`].
const CEILING_TEXT: usize = 2 * MAX_TEXT;

const MAX_PACKAGES: usize = 3;

const MAX_EXPORTS: usize = 3;

const MAX_REEXPORTS: usize = 3;

/// Applied to package names, export names, both fields of every `Reexport`,
/// and the name carried by `Query::Resolve` and `Query::PackageResolve`.
const MAX_NAME: usize = 32;

/// Apply the same limits to initial programs and [`Op::Edit`] replacements.
/// Statement and text ceilings allow overshoot of the mutation thresholds.
pub(super) fn within_bounds(scenario: &Scenario) -> anyhow::Result<()> {
    let files = scenario.initial.files.len();
    if files > MAX_FILES {
        return Err(anyhow::anyhow!(
            "the workspace has {files} files but mutation produces at most {MAX_FILES}"
        ));
    }

    let ops = scenario.ops.len();
    if ops > MAX_OPS {
        return Err(anyhow::anyhow!(
            "the history has {ops} operations but mutation produces at most {MAX_OPS}"
        ));
    }

    within_package_bounds(scenario)?;

    for (context, name) in sited_query_names(scenario) {
        within_name_bounds(name, &format!("{context} name"))?;
    }

    for (site, program) in sited_programs(scenario) {
        for stmt in &program.statements {
            let depth = height(stmt);
            if depth > MAX_DEPTH {
                return Err(anyhow::anyhow!(
                    "{} nests {depth} levels but mutation produces at most {MAX_DEPTH}",
                    site.render()
                ));
            }
        }

        let statements = count_statements(&program.statements);
        if statements > CEILING_STATEMENTS {
            return Err(anyhow::anyhow!(
                "{} has {statements} statements, over the {CEILING_STATEMENTS} ceiling",
                site.render()
            ));
        }

        let width = program.render().text.len();
        if width > CEILING_TEXT {
            return Err(anyhow::anyhow!(
                "{} renders {width} bytes, over the {CEILING_TEXT} ceiling",
                site.render()
            ));
        }
    }

    Ok(())
}

fn within_package_bounds(scenario: &Scenario) -> anyhow::Result<()> {
    let packages = scenario.initial.packages.len();
    if packages > MAX_PACKAGES {
        return Err(anyhow::anyhow!(
            "the workspace has {packages} packages but mutation produces at most {MAX_PACKAGES}"
        ));
    }

    for (index, package) in scenario.initial.packages.iter().enumerate() {
        let context = format!("package {index} name");
        within_name_bounds(&package.name, &context)?;
        within_identifier_charset(&package.name, &context)?;

        if package.exports.len() > MAX_EXPORTS {
            return Err(anyhow::anyhow!(
                "package {:?} has {} exports but mutation produces at most {MAX_EXPORTS}",
                package.name,
                package.exports.len()
            ));
        }
        for (export_index, export) in package.exports.iter().enumerate() {
            let context = format!("package {index} export {export_index}");
            within_name_bounds(export, &context)?;
            within_identifier_charset(export, &context)?;
        }

        if package.reexports.len() > MAX_REEXPORTS {
            return Err(anyhow::anyhow!(
                "package {:?} has {} reexports but mutation produces at most {MAX_REEXPORTS}",
                package.name,
                package.reexports.len()
            ));
        }
        for (reexport_index, reexport) in package.reexports.iter().enumerate() {
            let name_context = format!("package {index} reexport {reexport_index} name");
            within_name_bounds(&reexport.name, &name_context)?;
            within_identifier_charset(&reexport.name, &name_context)?;

            let from_context = format!("package {index} reexport {reexport_index} from");
            within_name_bounds(&reexport.from, &from_context)?;
            within_identifier_charset(&reexport.from, &from_context)?;
        }
    }

    Ok(())
}

fn within_name_bounds(name: &str, context: &str) -> anyhow::Result<()> {
    if name.len() > MAX_NAME {
        return Err(anyhow::anyhow!(
            "{context} is {} bytes, over the {MAX_NAME} byte limit",
            name.len()
        ));
    }
    Ok(())
}

fn within_identifier_charset(name: &str, context: &str) -> anyhow::Result<()> {
    if !is_identifier(name) {
        return Err(anyhow::anyhow!(
            "{context} {name:?} is not a valid identifier"
        ));
    }
    Ok(())
}

/// Query names go directly to `Name::new()` without parsing, so only their
/// size is restricted, not their spelling.
fn sited_query_names(scenario: &Scenario) -> Vec<(String, &str)> {
    let mut out = Vec::new();
    if let Some(name) = query_name(&scenario.cold_entry) {
        out.push(("cold_entry".to_string(), name));
    }
    for (index, op) in scenario.ops.iter().enumerate() {
        if let Op::Query(query) = op {
            if let Some(name) = query_name(query) {
                out.push((format!("op {index}"), name));
            }
        }
    }
    out
}

fn query_name(query: &Query) -> Option<&str> {
    match query {
        Query::Resolve(_, name) => Some(name.as_str()),
        Query::PackageResolve(_, name, _) => Some(name.as_str()),
        _ => None,
    }
}

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
            Step::AddReexportEdge => !packages_with_reexport_room(scenario).is_empty(),
            Step::RedirectReexportEdge | Step::RemoveReexportEdge => {
                !reexport_slots(scenario).is_empty()
            },
            Step::AddExport => !packages_with_export_room(scenario).is_empty(),
            Step::RemoveExport => !packages_with_exports(scenario).is_empty(),
            Step::AddPackage => scenario.initial.packages.len() < MAX_PACKAGES,
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
            Step::AddFile => add_file(scenario),
            Step::RemoveFile => remove_file(rng, scenario),
            Step::ChangeColdEntry => {
                scenario.cold_entry = random_query(rng, &Shape::of(&scenario.initial))
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
            Step::AddPackage => add_package(scenario),
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
    let op = if rng.odds(60) {
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
        Op::Query(query) => *query = random_query(rng, &Shape::of(&scenario.initial)),
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

const LIB_POOL: [&str; 3] = ["lib0", "lib1", "lib2"];

/// Combines `choose::export_name()`'s `exp_*` vocabulary with `val_*` binding
/// names, so a mutated export can either match another package's reexport or
/// a name a file actually binds.
fn export_vocabulary(rng: &mut impl Choose) -> String {
    if rng.odds(50) {
        export_name(rng.index(3))
    } else {
        binding_name(rng.index(3))
    }
}

fn add_export(rng: &mut impl Choose, scenario: &mut Scenario) {
    let Some(index) = pick(rng, packages_with_export_room(scenario)) else {
        return;
    };
    let export = export_vocabulary(rng);
    scenario.initial.packages[index].exports.push(export);
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
        .filter(|&index| scenario.initial.packages[index].exports.len() < MAX_EXPORTS)
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
        .filter(|&index| scenario.initial.packages[index].reexports.len() < MAX_REEXPORTS)
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

fn add_package(scenario: &mut Scenario) {
    if scenario.initial.packages.len() >= MAX_PACKAGES {
        return;
    }
    let existing: Vec<&str> = scenario
        .initial
        .packages
        .iter()
        .map(|package| package.name.as_str())
        .chain(scenario.initial.installed.iter().map(String::as_str))
        .collect();
    let Some(&name) = LIB_POOL.iter().find(|name| !existing.contains(name)) else {
        return;
    };
    scenario.initial.packages.push(PackageSpec {
        name: name.to_string(),
        kind: PackageKind::Library,
        exports: Vec::new(),
        reexports: Vec::new(),
    });
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

// == Scenario traversal ==

/// Address a statement by its program and index path through nested blocks.
#[derive(Clone, Debug)]
struct Slot {
    program: usize,
    path: Vec<usize>,
}

/// Include programs from the initial workspace and every edit replacement.
fn programs(scenario: &Scenario) -> Vec<&Program> {
    sited_programs(scenario)
        .into_iter()
        .map(|(_, program)| program)
        .collect()
}

/// Match the file and operation labels in the artifact when reporting a
/// rejected program.
enum ProgramSite {
    File(usize),
    Op(usize),
}

impl ProgramSite {
    fn render(&self) -> String {
        match self {
            ProgramSite::File(index) => format!("file [{index}]"),
            ProgramSite::Op(index) => format!("op {index}"),
        }
    }
}

/// Share the traversal between validation and mutation so both include edit
/// replacements in the same order.
fn sited_programs(scenario: &Scenario) -> Vec<(ProgramSite, &Program)> {
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
    if candidates.is_empty() || rng.odds(15) {
        return UNINSTALLED.to_string();
    }
    candidates[rng.index(candidates.len())].to_string()
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

    /// Check that repeated mutations keep initial and replacement programs
    /// within the limits imposed on decoded scenarios.
    #[test]
    fn test_mutation_respects_declared_bounds() {
        use mutatis::Session;

        use crate::fuzz::seed_corpus;

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
                assert!(count_statements(&program.statements) <= CEILING_STATEMENTS);
                assert!(program.render().text.len() <= CEILING_TEXT);
                for stmt in &program.statements {
                    assert!(height(stmt) <= MAX_DEPTH);
                }
            }

            assert!(within_bounds(scenario).is_ok());
        }
    }
}
