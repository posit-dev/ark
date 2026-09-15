//! Regression checks for mutation coverage, invariants, and shrinking.

use mutatis::Session;
use oak_semantic::fuzz::Program;
use oak_semantic::fuzz::Stmt;
use rand::rngs::StdRng;
use rand::SeedableRng;

use super::history::different_query;
use super::program::is_source;
use super::program::nest_statement;
use super::program::shadow_callee;
use super::program::unnest_statement;
use super::workspace::add_export;
use super::workspace::add_file;
use super::workspace::add_package;
use super::workspace::remove_package;
use super::ScenarioMutator;
use super::Step;
use super::STEPS;
use crate::fuzz::budgets::MAX_DEPTH;
use crate::fuzz::budgets::MAX_EXPORTS;
use crate::fuzz::budgets::MAX_FILES;
use crate::fuzz::budgets::MAX_OPS;
use crate::fuzz::budgets::MAX_PACKAGES;
use crate::fuzz::budgets::MAX_REEXPORTS;
use crate::fuzz::budgets::MAX_STATEMENTS;
use crate::fuzz::build::binding;
use crate::fuzz::build::function_def;
use crate::fuzz::build::qualified_source;
use crate::fuzz::build::shadow;
use crate::fuzz::build::source;
use crate::fuzz::choose::random_query;
use crate::fuzz::choose::Choose;
use crate::fuzz::choose::Shape;
use crate::fuzz::corpus;
use crate::fuzz::limits::within_bounds;
use crate::fuzz::scenario::Query;
use crate::fuzz::scenario::Scenario;
use crate::fuzz::seed_corpus;
use crate::fuzz::spec::FileId;
use crate::fuzz::spec::Owner;
use crate::fuzz::spec::PackageId;
use crate::fuzz::spec::PackageKind;
use crate::fuzz::traversal::count_statements;
use crate::fuzz::traversal::height;
use crate::fuzz::traversal::programs;
use crate::fuzz::traversal::slots_where;

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
