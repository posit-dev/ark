//! Regression checks for mutation coverage, invariants, and shrinking.

use mutatis::Session;
use oak_semantic::effects::fuzz::SourceProvider;
use oak_semantic::fuzz::Program;
use oak_semantic::fuzz::Stmt;
use rand::rngs::StdRng;
use rand::SeedableRng;

use super::history::different_query;
use super::program::is_source;
use super::program::nest_statement;
use super::program::redirect_source_edge;
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
use crate::fuzz::scenario::Op;
use crate::fuzz::scenario::Query;
use crate::fuzz::scenario::Scenario;
use crate::fuzz::seed_corpus;
use crate::fuzz::spec::FileId;
use crate::fuzz::spec::FileSpec;
use crate::fuzz::spec::Owner;
use crate::fuzz::spec::PackageId;
use crate::fuzz::spec::PackageKind;
use crate::fuzz::spec::PackageSpec;
use crate::fuzz::spec::WorkspaceSpec;
use crate::fuzz::targets::SourceCandidates;
use crate::fuzz::traversal::count_statements;
use crate::fuzz::traversal::height;
use crate::fuzz::traversal::programs;
use crate::fuzz::traversal::slots_where;
use crate::fuzz::World;

/// Number of fixed-seed blocks exercised by the opt-in suite.
const BLOCK_SEEDS: u64 = 6;

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

        // Full validation includes replay limits and structural invariants, so
        // every mutated failure can be decoded for replay.
        assert!(scenario.validate().is_ok());
    }
}

// == Context-aware source targets ==

/// Includes a nested file so candidate selection can distinguish files from
/// directories.
fn script_layout() -> WorkspaceSpec {
    WorkspaceSpec {
        installed: vec!["base".to_string(), "targets".to_string()],
        packages: vec![],
        files: vec![
            file_spec(Owner::Script, "a.R"),
            file_spec(Owner::Script, "b.R"),
            file_spec(Owner::Script, "sub/c.R"),
        ],
    }
}

fn package_layout() -> WorkspaceSpec {
    WorkspaceSpec {
        installed: vec!["base".to_string(), "targets".to_string()],
        packages: vec![PackageSpec {
            name: "mypkg".to_string(),
            kind: PackageKind::Workspace,
            exports: Vec::new(),
            reexports: Vec::new(),
        }],
        files: vec![
            file_spec(Owner::Package(PackageId(0)), "R/a.R"),
            file_spec(Owner::Package(PackageId(0)), "R/b.R"),
            file_spec(Owner::Package(PackageId(0)), "R/sub/c.R"),
        ],
    }
}

fn file_spec(owner: Owner, path: &str) -> FileSpec {
    FileSpec {
        owner,
        path: path.to_string(),
        program: Program {
            statements: vec![binding("val")],
        },
    }
}

/// `anchor_dir()` resolves against the calling file's root, so paths owned only
/// by another root must not count as local candidates.
#[test]
fn test_source_candidates_separate_roots_and_kinds() {
    let mut spec = script_layout();
    spec.packages = package_layout().packages;
    spec.files.extend(package_layout().files);

    let scripts = SourceCandidates::for_owner(&spec, Owner::Script);
    assert!(scripts.accepts(SourceProvider::File, "a.R"));
    assert!(scripts.accepts(SourceProvider::File, "sub/c.R"));
    assert!(scripts.accepts(SourceProvider::Dir, "."));
    assert!(scripts.accepts(SourceProvider::Dir, "sub"));
    assert!(!scripts.accepts(SourceProvider::File, "R/a.R"));
    assert!(!scripts.accepts(SourceProvider::Dir, "R"));
    assert!(!scripts.accepts(SourceProvider::File, "."));

    let package = SourceCandidates::for_owner(&spec, Owner::Package(PackageId(0)));
    assert!(package.accepts(SourceProvider::File, "R/a.R"));
    assert!(package.accepts(SourceProvider::Dir, "R"));
    assert!(package.accepts(SourceProvider::Dir, "R/sub"));
    assert!(!package.accepts(SourceProvider::File, "a.R"));
    assert!(package.accepts(SourceProvider::FileOrDir, "R/sub/c.R"));
    assert!(package.accepts(SourceProvider::FileOrDir, "R/sub"));
}

/// Checks drawn paths with the production resolver rather than trusting the
/// candidate pool's own classification.
#[test]
fn test_drawn_file_targets_resolve_in_both_layouts() {
    for (owner, spec) in [
        (Owner::Script, script_layout()),
        (Owner::Package(PackageId(0)), package_layout()),
    ] {
        let candidates = SourceCandidates::for_owner(&spec, owner);
        let mut rng = StdRng::seed_from_u64(0);

        for _ in 0..30 {
            let target = candidates.resolvable(&mut rng, SourceProvider::File);
            let expected = match spec.ids().find(|&id| spec.file(id).path == target) {
                Some(id) => spec.absolute_path(id),
                None => panic!("drawn file target {target} names no file"),
            };
            // The probe file sources the draw, so drawing the probe itself
            // forms a cycle and `NoopImportsResolver` leaves no edge to read.
            if target == spec.file(FileId(0)).path {
                continue;
            }

            let mut probe = spec.clone();
            probe.files[0].program = Program {
                statements: vec![source(&target)],
            };
            let world = World::materialize(&probe);
            assert_eq!(world.source_targets(FileId(0)), [expected]);
        }
    }
}

/// Keeps both resolvable and unresolvable paths reachable.
#[test]
fn test_source_candidates_retain_unresolvable_draws() {
    let spec = script_layout();
    let candidates = SourceCandidates::for_owner(&spec, Owner::Script);
    let mut rng = StdRng::seed_from_u64(0);

    let unresolvable = (0..400)
        .map(|_| candidates.target(&mut rng, SourceProvider::File))
        .filter(|target| !candidates.accepts(SourceProvider::File, target))
        .count();
    assert!(unresolvable > 0);
    assert!(unresolvable < 400);
}

// == Edits reaching analysis ==

/// Most inserted edits should be followed by a query that observes them.
#[test]
fn test_inserted_edits_are_usually_paired_with_an_observing_query() {
    let mut rng = StdRng::seed_from_u64(0);
    let mut edits = 0;
    let mut paired = 0;

    for _ in 0..200 {
        let mut scenario = corpus::case("acyclic_pair_closes_then_reopens");
        scenario.ops.clear();
        Step::InsertOp.apply(&mut rng, &mut scenario);

        let Some(Op::Edit(edit)) = scenario.ops.first() else {
            continue;
        };
        edits += 1;
        if matches!(scenario.ops.get(1), Some(Op::Query(query)) if query.file() == Some(edit.file))
        {
            paired += 1;
        }
    }

    assert!(edits > 0);
    assert!(paired * 2 > edits);
}

/// Generated histories should usually observe replacements without eliminating
/// intentionally unobserved `touch()` edits.
#[test]
fn test_seed_histories_usually_query_the_edited_file() {
    let mut edits = 0;
    let mut observed = 0;

    for seed in 0..BLOCK_SEEDS {
        for scenario in seed_corpus(seed) {
            for (index, op) in scenario.ops.iter().enumerate() {
                let Op::Edit(edit) = op else {
                    continue;
                };
                edits += 1;
                let Some(Op::Query(query)) = scenario.ops.get(index + 1) else {
                    continue;
                };
                if query.file() == Some(edit.file) {
                    observed += 1;
                }
            }
        }
    }

    assert!(edits > 0);
    // Intentionally unobserved `touch()` edits keep the expected rate below one,
    // but paired post-edit queries must still contribute substantially.
    assert!(observed * 5 > edits * 2);
}

/// A redirect must change a lone self-source rather than report a successful
/// no-op when no other local file exists.
#[test]
fn test_redirect_moves_a_lone_self_source() {
    let mut scenario = scenario_with(vec![source("a.R")]);
    scenario.initial.files[0].path = "a.R".to_string();
    let mut rng = StdRng::seed_from_u64(0);

    for _ in 0..20 {
        let before = scenario.initial.files[0].program.render().text;
        redirect_source_edge(&mut rng, &mut scenario);
        assert_ne!(scenario.initial.files[0].program.render().text, before);
        // Restore the self-source so every iteration exercises the collision fallback.
        scenario.initial.files[0].program = Program {
            statements: vec![source("a.R")],
        };
    }
}
