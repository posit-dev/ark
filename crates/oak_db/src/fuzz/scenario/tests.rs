//! Scenario serialization and validation regression tests.

use mutatis::Session;
use oak_semantic::effects::fuzz::EffectRecipe;
use oak_semantic::fuzz::Expr;
use oak_semantic::fuzz::Invocation;
use oak_semantic::fuzz::Program;
use oak_semantic::fuzz::Stmt;

use super::Edit;
use super::Op;
use super::Query;
use super::Scenario;
use crate::fuzz::build::binding;
use crate::fuzz::build::function_def;
use crate::fuzz::build::source;
use crate::fuzz::corpus;
use crate::fuzz::corpus::corpus;
use crate::fuzz::seed_corpus;
use crate::fuzz::spec::FileId;
use crate::fuzz::spec::Owner;
use crate::fuzz::spec::PackageId;
use crate::fuzz::spec::Reexport;
use crate::fuzz::spec::WorkspaceSpec;
use crate::fuzz::ScenarioMutator;
use crate::NamespaceVisibility;

#[test]
fn test_corpus_scenarios_round_trip() {
    for case in corpus() {
        let json = case.scenario.to_json().unwrap();
        let restored = Scenario::from_json(json.as_bytes()).unwrap();
        assert_eq!(restored.to_json().unwrap(), json);
        // JSON equality misses fields omitted by serialization. Compare the
        // rendered scenario too, so omissions that change its output fail.
        assert_eq!(restored.header(), case.scenario.header());
        assert_eq!(restored.render(), case.scenario.render());
    }
}

/// Exercise combinations beyond the fixed corpus. The separate
/// [`test_nested_hole_round_trips()`] test guarantees coverage of a source
/// call inside a quotation hole, regardless of which mutations are chosen.
#[test]
fn test_mutated_scenarios_round_trip() {
    const MUTATIONS: usize = 300;

    let mut session = Session::new().seed(0);
    let mut corpus = seed_corpus(0);

    for round in 0..MUTATIONS {
        let entry = round % corpus.len();
        if session
            .mutate_with(&mut ScenarioMutator, &mut corpus[entry])
            .is_err()
        {
            continue;
        }

        assert!(corpus[entry].validate().is_ok());

        let json = corpus[entry].to_json().unwrap();
        let restored = Scenario::from_json(json.as_bytes()).unwrap();
        assert_eq!(restored.to_json().unwrap(), json);
    }
}

#[test]
fn test_nested_hole_round_trips() {
    let mut scenario = corpus::case("acyclic_pair_closes_then_reopens");
    scenario.initial.files[0].program = Program {
        statements: vec![Stmt::effect(
            EffectRecipe::QuoteHoles {
                body: vec![Stmt::Expr(Expr::Hole(vec![source("b.R")]))],
            },
            Invocation::Bare,
        )],
    };
    assert!(scenario.render().contains("bquote(.(source(\"b.R\")))"));

    let json = scenario.to_json().unwrap();
    let restored = Scenario::from_json(json.as_bytes()).unwrap();

    assert_eq!(restored.render(), scenario.render());
}

#[test]
fn test_malformed_input_is_an_error() {
    assert!(Scenario::from_json(b"not json").is_err());
    assert!(Scenario::from_json(b"{}").is_err());
    assert!(Scenario::from_json(b"[]").is_err());
}

#[test]
fn test_out_of_range_cold_entry_is_rejected() {
    let mut scenario = corpus::case("acyclic_pair_closes_then_reopens");
    scenario.cold_entry = Query::Diagnostics(FileId(2));

    let error = scenario.validate().unwrap_err();
    assert_eq!(
        error.to_string(),
        "cold_entry references file 2 but the workspace has 2 files"
    );
}

#[test]
fn test_oversized_workspace_is_rejected() {
    let mut scenario = corpus::case("acyclic_pair_closes_then_reopens");
    let file = scenario.initial.files[1].clone();
    while scenario.initial.files.len() <= 5 {
        scenario.initial.files.push(file.clone());
    }

    let error = scenario.validate().unwrap_err();
    assert_eq!(
        error.to_string(),
        "the workspace has 6 files but replay accepts at most 5"
    );
}

/// Replacement programs need the same bounds as initial files because the
/// runner renders and installs them after the cold query.
#[test]
fn test_oversized_edit_replacement_is_rejected() {
    let deep = function_def("outer", vec![function_def("middle", vec![function_def(
        "inner",
        vec![binding("val")],
    )])]);
    let wide = Program {
        statements: (0..200)
            .map(|index| binding(&format!("val_{index}")))
            .collect(),
    };

    let mut nested = corpus::case("acyclic_pair_closes_then_reopens");
    nested.ops.push(Op::Edit(Edit {
        file: FileId(0),
        program: Program {
            statements: vec![deep],
        },
    }));
    let error = nested.validate().unwrap_err();
    assert_eq!(
        error.to_string(),
        "op 2 nests 4 levels but replay accepts at most 3"
    );

    let mut widened = corpus::case("acyclic_pair_closes_then_reopens");
    widened.ops.push(Op::Edit(Edit {
        file: FileId(0),
        program: wide,
    }));
    let error = widened.validate().unwrap_err();
    assert_eq!(
        error.to_string(),
        "op 2 has 200 statements, over the 28 ceiling"
    );
}

/// A file-less workspace passes the file-id checks when every operation is
/// an aggregate query, but mutation then draws file 0 and indexes nothing.
#[test]
fn test_empty_workspace_is_rejected() {
    let scenario = Scenario {
        seed: 0,
        variant: 0,
        initial: WorkspaceSpec {
            installed: vec!["base".to_string()],
            packages: vec![],
            files: vec![],
        },
        cold_entry: Query::AllPackageDependencies,
        ops: vec![],
    };

    let json = scenario.to_json().unwrap();
    let error = Scenario::from_json(json.as_bytes()).unwrap_err();
    assert_eq!(error.to_string(), "the workspace has no files");
}

#[test]
fn test_out_of_range_op_file_is_rejected() {
    let mut scenario = corpus::case("acyclic_pair_closes_then_reopens");
    scenario.ops.push(Op::Query(Query::Imports(FileId(9))));

    let error = scenario.validate().unwrap_err();
    assert_eq!(
        error.to_string(),
        "op 2 references file 9 but the workspace has 2 files"
    );
}

#[test]
fn test_package_owned_file_with_out_of_range_package_is_rejected() {
    let mut scenario = corpus::case("acyclic_pair_closes_then_reopens");
    scenario.initial.files[0].owner = Owner::Package(PackageId(0));

    let error = scenario.validate().unwrap_err();
    assert_eq!(
        error.to_string(),
        "file a.R owner references package 0 but the workspace has 0 packages"
    );
}

#[test]
fn test_unresolved_source_path_stays_valid() {
    let mut scenario = corpus::case("acyclic_pair_closes_then_reopens");
    scenario.initial.files[0].program = Program {
        statements: vec![source("missing.R")],
    };

    assert!(scenario.validate().is_ok());
}

#[test]
fn test_corpus_and_seed_corpus_validate() {
    for case in corpus() {
        assert!(case.scenario.validate().is_ok());
    }
    for scenario in seed_corpus(0) {
        assert!(scenario.validate().is_ok());
    }
}

#[test]
fn test_out_of_range_package_id_is_rejected() {
    let mut scenario = corpus::case("acyclic_reexport_chain_resolves_to_the_definition");
    scenario.cold_entry = Query::PackageResolve(
        PackageId(5),
        "exp_a".to_string(),
        NamespaceVisibility::Exported,
    );

    let error = scenario.validate().unwrap_err();
    assert_eq!(
        error.to_string(),
        "cold_entry references package 5 but the workspace has 2 packages"
    );
}

#[test]
fn test_file_owned_by_library_package_is_rejected() {
    let mut scenario = corpus::case("mutual_reexport_has_no_terminal_definition");
    scenario.initial.files[0].owner = Owner::Package(PackageId(0));

    let error = scenario.validate().unwrap_err();
    assert_eq!(
        error.to_string(),
        "file a.R is owned by library package lib0"
    );
}

#[test]
fn test_duplicate_package_name_is_rejected() {
    let mut scenario = corpus::case("acyclic_reexport_chain_resolves_to_the_definition");
    let mut duplicate = scenario.initial.packages[1].clone();
    duplicate.name = scenario.initial.packages[0].name.clone();
    scenario.initial.packages.push(duplicate);

    let error = scenario.validate().unwrap_err();
    assert_eq!(
        error.to_string(),
        "package name \"pkga\" is used more than once"
    );
}

#[test]
fn test_base_named_package_is_rejected() {
    let mut scenario = corpus::case("acyclic_reexport_chain_resolves_to_the_definition");
    scenario.initial.packages[0].name = "base".to_string();

    let error = scenario.validate().unwrap_err();
    assert_eq!(error.to_string(), "package cannot be named \"base\"");
}

#[test]
fn test_over_max_packages_is_rejected() {
    let mut scenario = corpus::case("acyclic_reexport_chain_resolves_to_the_definition");
    let extra = scenario.initial.packages[1].clone();
    for index in 0..3 {
        let mut package = extra.clone();
        package.name = format!("pkg{index}");
        scenario.initial.packages.push(package);
    }

    let error = scenario.validate().unwrap_err();
    assert_eq!(
        error.to_string(),
        "the workspace has 5 packages but replay accepts at most 3"
    );
}

#[test]
fn test_over_max_exports_is_rejected() {
    let mut scenario = corpus::case("acyclic_reexport_chain_resolves_to_the_definition");
    scenario.initial.packages[1].exports = vec![
        "exp_a".to_string(),
        "exp_b".to_string(),
        "exp_c".to_string(),
        "exp_d".to_string(),
    ];

    let error = scenario.validate().unwrap_err();
    assert_eq!(
        error.to_string(),
        "package \"pkgb\" has 4 exports but replay accepts at most 3"
    );
}

#[test]
fn test_over_max_reexports_is_rejected() {
    let mut scenario = corpus::case("acyclic_reexport_chain_resolves_to_the_definition");
    scenario.initial.packages[0].reexports = vec![
        Reexport {
            name: "exp_a".to_string(),
            from: "pkgb".to_string(),
        },
        Reexport {
            name: "exp_b".to_string(),
            from: "pkgb".to_string(),
        },
        Reexport {
            name: "exp_c".to_string(),
            from: "pkgb".to_string(),
        },
        Reexport {
            name: "exp_d".to_string(),
            from: "pkgb".to_string(),
        },
    ];

    let error = scenario.validate().unwrap_err();
    assert_eq!(
        error.to_string(),
        "package \"pkga\" has 4 reexports but replay accepts at most 3"
    );
}

#[test]
fn test_oversized_package_name_is_rejected() {
    let mut scenario = corpus::case("acyclic_reexport_chain_resolves_to_the_definition");
    scenario.initial.packages[0].name = "a".repeat(40);

    let error = scenario.validate().unwrap_err();
    assert_eq!(
        error.to_string(),
        "package 0 name is 40 bytes, over the 32 byte limit"
    );
}

#[test]
fn test_oversized_export_name_is_rejected() {
    let mut scenario = corpus::case("acyclic_reexport_chain_resolves_to_the_definition");
    scenario.initial.packages[1].exports.push("b".repeat(40));

    let error = scenario.validate().unwrap_err();
    assert_eq!(
        error.to_string(),
        "package 1 export 1 is 40 bytes, over the 32 byte limit"
    );
}

#[test]
fn test_oversized_reexport_name_is_rejected() {
    let mut scenario = corpus::case("acyclic_reexport_chain_resolves_to_the_definition");
    scenario.initial.packages[0].reexports[0].name = "c".repeat(40);

    let error = scenario.validate().unwrap_err();
    assert_eq!(
        error.to_string(),
        "package 0 reexport 0 name is 40 bytes, over the 32 byte limit"
    );
}

#[test]
fn test_oversized_reexport_source_is_rejected() {
    let mut scenario = corpus::case("acyclic_reexport_chain_resolves_to_the_definition");
    scenario.initial.packages[0].reexports[0].from = "d".repeat(40);

    let error = scenario.validate().unwrap_err();
    assert_eq!(
        error.to_string(),
        "package 0 reexport 0 from is 40 bytes, over the 32 byte limit"
    );
}

#[test]
fn test_oversized_package_resolve_name_is_rejected() {
    let mut scenario = corpus::case("acyclic_reexport_chain_resolves_to_the_definition");
    scenario.cold_entry =
        Query::PackageResolve(PackageId(0), "e".repeat(40), NamespaceVisibility::Exported);

    let error = scenario.validate().unwrap_err();
    assert_eq!(
        error.to_string(),
        "cold_entry name is 40 bytes, over the 32 byte limit"
    );
}

#[test]
fn test_non_identifier_export_name_is_rejected() {
    let mut scenario = corpus::case("acyclic_reexport_chain_resolves_to_the_definition");
    scenario.initial.packages[1]
        .exports
        .push("1bad".to_string());

    let error = scenario.validate().unwrap_err();
    assert_eq!(
        error.to_string(),
        "package 1 export 1 \"1bad\" is not a valid identifier"
    );
}

/// `is_identifier()` admits reserved words, so the boundary has to consult
/// the parser itself. Otherwise the materializer meets NAMESPACE text that
/// cannot parse and panics on an input the driver can reach.
#[test]
fn test_reserved_word_export_is_rejected() {
    let mut scenario = corpus::case("acyclic_reexport_chain_resolves_to_the_definition");
    scenario.initial.packages[1].exports[0] = "if".to_string();

    let error = scenario.validate().unwrap_err();
    assert!(error
        .to_string()
        .starts_with("package pkgb renders a NAMESPACE that does not parse:"));
}

#[test]
fn test_reserved_word_reexport_source_is_rejected() {
    let mut scenario = corpus::case("acyclic_reexport_chain_resolves_to_the_definition");
    scenario.initial.packages[0].reexports[0].from = "function".to_string();

    let error = scenario.validate().unwrap_err();
    assert!(error
        .to_string()
        .starts_with("package pkga renders a NAMESPACE that does not parse:"));
}

/// A literal parses but is not an identifier, so the directive silently
/// carries no name at all.
#[test]
fn test_literal_export_name_is_rejected() {
    let mut scenario = corpus::case("acyclic_reexport_chain_resolves_to_the_definition");
    scenario.initial.packages[1].exports[0] = "TRUE".to_string();

    let error = scenario.validate().unwrap_err();
    assert_eq!(
        error.to_string(),
        "package pkgb exports TRUE, which the NAMESPACE parser reads as no name"
    );
}

#[test]
fn test_literal_reexport_source_is_rejected() {
    let mut scenario = corpus::case("acyclic_reexport_chain_resolves_to_the_definition");
    scenario.initial.packages[0].reexports[0].from = "NULL".to_string();

    let error = scenario.validate().unwrap_err();
    assert_eq!(
        error.to_string(),
        "package pkga imports exp_a from NULL, which the NAMESPACE parser reads as no name"
    );
}

/// The parser keeps one `importFrom` per name, so a spec carrying two
/// would describe an edge the database does not have.
#[test]
fn test_duplicate_reexport_name_is_rejected() {
    let mut scenario = corpus::case("acyclic_reexport_chain_resolves_to_the_definition");
    let duplicate = Reexport {
        name: scenario.initial.packages[0].reexports[0].name.clone(),
        from: "pkgz".to_string(),
    };
    scenario.initial.packages[0].reexports.push(duplicate);

    let error = scenario.validate().unwrap_err();
    assert_eq!(
        error.to_string(),
        "package pkga re-exports exp_a more than once"
    );
}
