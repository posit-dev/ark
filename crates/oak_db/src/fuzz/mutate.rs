//! Semantic mutations over a [`Scenario`].
//!
//! A derived mutator would waste checks on numeric digits and invalid
//! identifiers. These mutations instead change source edges, call resolution,
//! statement nesting, files, and edit history.
//!
//! Growth thresholds are checked while selecting candidates because `Check`
//! repeatedly mutates its generated corpus. Candidate filters also avoid common
//! no-op mutations.
//!
//! `program` handles source calls and statement structure, `workspace` handles
//! files and packages, and `history` handles edit and query operations.

mod history;
mod program;
mod workspace;

#[cfg(test)]
mod tests;

use mutatis::Candidates;
use mutatis::Mutate;
use mutatis::Result;

use self::history::alter_op;
use self::history::alterable_ops;
use self::history::different_query;
use self::history::insert_op;
use self::program::add_source_edge;
use self::program::flip_invocation;
use self::program::insert_statement;
use self::program::insertable_block_slots;
use self::program::is_flippable;
use self::program::is_source;
use self::program::nest_statement;
use self::program::nestable_slots;
use self::program::redirect_source_edge;
use self::program::remove_slot;
use self::program::reorder_statements;
use self::program::reorderable_block_slots;
use self::program::shadow_callee;
use self::program::shadowable_slots;
use self::program::swap_provider;
use self::program::unnest_statement;
use self::workspace::add_export;
use self::workspace::add_file;
use self::workspace::add_package;
use self::workspace::add_reexport_edge;
use self::workspace::available_package_names;
use self::workspace::packages_with_export_room;
use self::workspace::packages_with_exports;
use self::workspace::packages_with_reexport_room;
use self::workspace::redirect_reexport_edge;
use self::workspace::reexport_slots;
use self::workspace::removable_packages;
use self::workspace::remove_export;
use self::workspace::remove_file;
use self::workspace::remove_package;
use self::workspace::remove_reexport_edge;
use crate::fuzz::budgets::MAX_FILES;
use crate::fuzz::budgets::MAX_OPS;
use crate::fuzz::budgets::MAX_PACKAGES;
use crate::fuzz::choose::Choose;
use crate::fuzz::choose::Shape;
use crate::fuzz::scenario::Scenario;
use crate::fuzz::traversal::slots_where;

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

fn pick<T>(rng: &mut impl Choose, items: Vec<T>) -> Option<T> {
    if items.is_empty() {
        return None;
    }
    let index = rng.index(items.len());
    items.into_iter().nth(index)
}
