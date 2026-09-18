//! Regression checks for mutation coverage, invariants, and shrinking.

use mutatis::Session;
use oak_semantic::effects::fuzz::SourceProvider;
use oak_semantic::fuzz::Program;
use oak_semantic::fuzz::Stmt;
use rand::rngs::StdRng;
use rand::SeedableRng;

use super::history::different_query;
use super::history::pending_replacements;
use super::history::settleable_queries;
use super::program::is_source;
use super::program::nest_statement;
use super::program::redirect_source_edge;
use super::program::shadow_callee;
use super::program::shadowable_slots;
use super::program::unnest_statement;
use super::workspace::add_export;
use super::workspace::add_file;
use super::workspace::add_package;
use super::workspace::remove_package;
use super::ScenarioMutator;
use super::Step;
use super::STEPS;
use crate::file_imports::CollationView;
use crate::fuzz::budgets::MAX_DEPTH;
use crate::fuzz::budgets::MAX_EXPORTS;
use crate::fuzz::budgets::MAX_FILES;
use crate::fuzz::budgets::MAX_OPS;
use crate::fuzz::budgets::MAX_PACKAGES;
use crate::fuzz::budgets::MAX_REEXPORTS;
use crate::fuzz::budgets::MAX_STATEMENTS;
use crate::fuzz::build::binding;
use crate::fuzz::build::function_def;
use crate::fuzz::build::library;
use crate::fuzz::build::qualified_source;
use crate::fuzz::build::shadow;
use crate::fuzz::build::source;
use crate::fuzz::choose::random_query;
use crate::fuzz::choose::Choose;
use crate::fuzz::choose::Shape;
use crate::fuzz::corpus;
use crate::fuzz::scenario::Edit;
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

/// Exercise the history-repair branch directly, without asserting how often
/// random sampling happens to select it.
struct PreferObserver;

impl Choose for PreferObserver {
    fn index(&mut self, len: usize) -> usize {
        assert!(len > 0);
        0
    }

    fn odds(&mut self, _percent: u32) -> bool {
        true
    }
}

fn scenario_with(statements: Vec<Stmt>) -> Scenario {
    let mut scenario = corpus::scenario("acyclic_pair_closes_then_reopens");
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
    let mut scenario = corpus::scenario("acyclic_pair_closes_then_reopens");
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

// == History repair ==

/// Naming the edited file is not enough, and observing an earlier replacement
/// must not settle a later edit of that file.
#[test]
fn test_pending_replacements_require_a_later_direct_observer() {
    let mut scenario = corpus::scenario("acyclic_pair_closes_then_reopens");
    scenario.ops = vec![
        edit_of(&scenario, FileId(0)),
        Op::Query(Query::Diagnostics(FileId(1))),
        Op::Query(Query::CrossFileLayers(FileId(0), CollationView::Eager)),
        Op::Query(Query::AllWorkspacePackageDependencies),
        Op::Query(Query::Diagnostics(FileId(0))),
        edit_of(&scenario, FileId(0)),
    ];

    assert_eq!(pending_replacements(&scenario, 4), [FileId(0)]);
    assert!(pending_replacements(&scenario, 5).is_empty());
    assert_eq!(pending_replacements(&scenario, 6), [FileId(0)]);
}

/// An inserted direct observer can repair a replacement left pending anywhere
/// earlier in the history.
#[test]
fn test_inserted_queries_settle_a_pending_replacement() -> anyhow::Result<()> {
    let mut scenario = corpus::scenario("acyclic_pair_closes_then_reopens");
    scenario.ops = vec![
        edit_of(&scenario, FileId(0)),
        edit_of(&scenario, FileId(1)),
        Op::Query(Query::Diagnostics(FileId(0))),
    ];
    let before = scenario.clone();

    Step::InsertOp.apply(&mut PreferObserver, &mut scenario);

    assert_eq!(scenario.ops.len(), before.ops.len() + 1);
    assert!(scenario.validate().is_ok());
    assert!(pending_replacements(&scenario, scenario.ops.len()).is_empty());
    let Some(Op::Query(query)) = scenario.ops.pop() else {
        panic!("repair did not append a query");
    };
    assert_eq!(query.file(), Some(FileId(1)));
    assert_eq!(scenario.to_json()?, before.to_json()?);
    Ok(())
}

/// A full history can still be repaired when a query follows the pending edit.
#[test]
fn test_alteration_settles_a_pending_replacement_at_the_operation_limit() {
    let mut scenario = corpus::scenario("acyclic_pair_closes_then_reopens");
    scenario.ops = (0..MAX_OPS - 2)
        .map(|_| Op::Query(Query::Diagnostics(FileId(1))))
        .collect();
    scenario.ops.push(edit_of(&scenario, FileId(0)));
    scenario
        .ops
        .push(Op::Query(Query::AllWorkspacePackageDependencies));
    assert!(!Step::InsertOp.applies(&scenario));

    Step::AlterOp.apply(&mut PreferObserver, &mut scenario);

    assert_eq!(scenario.ops.len(), MAX_OPS);
    assert!(scenario.validate().is_ok());
    assert!(pending_replacements(&scenario, scenario.ops.len()).is_empty());
    assert!(
        matches!(scenario.ops.last(), Some(Op::Query(query)) if query.file() == Some(FileId(0)))
    );
}

/// Retargeting must not settle one replacement by removing another's only
/// direct observer.
#[test]
fn test_alteration_does_not_exchange_one_pending_replacement_for_another() {
    let mut scenario = corpus::scenario("acyclic_pair_closes_then_reopens");
    scenario.ops = vec![
        edit_of(&scenario, FileId(0)),
        edit_of(&scenario, FileId(1)),
        Op::Query(Query::Diagnostics(FileId(0))),
    ];
    assert!(settleable_queries(&scenario).is_empty());

    // The aggregate is not a direct observer, so either replacement is safe.
    scenario.ops[2] = Op::Query(Query::AllWorkspaceFileDependencies);
    assert_eq!(settleable_queries(&scenario), [
        (2, FileId(0)),
        (2, FileId(1))
    ]);
}

/// A settled history offers no repair because every replacement already has a
/// later direct observer.
#[test]
fn test_a_settled_history_offers_no_retargeting_repair() {
    let mut scenario = corpus::scenario("acyclic_pair_closes_then_reopens");
    scenario.ops = vec![
        edit_of(&scenario, FileId(0)),
        edit_of(&scenario, FileId(1)),
        Op::Query(Query::Diagnostics(FileId(0))),
        Op::Query(Query::Diagnostics(FileId(1))),
    ];

    assert!(pending_replacements(&scenario, scenario.ops.len()).is_empty());
    assert!(settleable_queries(&scenario).is_empty());

    // Ordinary alteration stays free to remove an observation.
    Step::AlterOp.apply(&mut FixedChoice(0), &mut scenario);
    assert!(scenario.validate().is_ok());
}

/// A query before the final replacement observes only an intermediate program,
/// so it cannot settle that file.
#[test]
fn test_repair_skips_a_position_before_a_later_replacement() {
    let mut scenario = corpus::scenario("acyclic_pair_closes_then_reopens");
    scenario.ops = vec![
        edit_of(&scenario, FileId(0)),
        Op::Query(Query::AllWorkspaceFileDependencies),
        edit_of(&scenario, FileId(0)),
        Op::Query(Query::AllWorkspaceFileDependencies),
    ];
    assert_eq!(pending_replacements(&scenario, scenario.ops.len()), [
        FileId(0)
    ]);

    assert_eq!(settleable_queries(&scenario), [(3, FileId(0))]);

    Step::AlterOp.apply(&mut PreferObserver, &mut scenario);

    assert_eq!(scenario.ops.len(), 4);
    assert!(scenario.validate().is_ok());
    assert!(pending_replacements(&scenario, scenario.ops.len()).is_empty());
}

/// No existing query can observe a replacement that occurs after every query.
#[test]
fn test_a_full_history_with_a_final_edit_has_no_retargeting_repair() {
    let mut scenario = corpus::scenario("acyclic_pair_closes_then_reopens");
    scenario.ops = (0..MAX_OPS - 1)
        .map(|_| Op::Query(Query::Diagnostics(FileId(0))))
        .collect();
    scenario.ops.push(edit_of(&scenario, FileId(0)));

    assert_eq!(pending_replacements(&scenario, MAX_OPS), [FileId(0)]);
    assert!(settleable_queries(&scenario).is_empty());
    assert!(!Step::InsertOp.applies(&scenario));
    assert!(scenario.validate().is_ok());
}

fn edit_of(scenario: &Scenario, file: FileId) -> Op {
    Op::Edit(Edit {
        file,
        program: scenario.initial.file(file).program.clone(),
    })
}

/// Do not spend the statement budget on a duplicate shadow in the same block.
#[test]
fn test_shadow_is_not_offered_for_an_already_shadowed_call() {
    let mut scenario = scenario_with(vec![source("b.R")]);
    assert!(Step::ShadowCallee.applies(&scenario));

    shadow_callee(&mut FixedChoice(0), &mut scenario);
    let expected = Program {
        statements: vec![shadow("source"), source("b.R")],
    };
    assert_eq!(
        scenario.initial.files[0].program.render().text,
        expected.render().text
    );
    assert!(!Step::ShadowCallee.applies(&scenario));

    // A different callee still needs its own binding.
    let other = scenario_with(vec![shadow("source"), source("b.R"), library("pkga")]);
    assert_eq!(shadowable_slots(&other).len(), 1);

    // A later binding does not suppress the call.
    let after = scenario_with(vec![source("b.R"), shadow("source")]);
    assert!(Step::ShadowCallee.applies(&after));

    // Enclosing bindings are outside this local duplicate check.
    let nested = scenario_with(vec![
        shadow("source"),
        function_def("fun", vec![source("b.R")]),
    ]);
    assert_eq!(shadowable_slots(&nested).len(), 1);
}
