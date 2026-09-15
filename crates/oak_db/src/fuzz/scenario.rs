//! Stores concrete scenarios without Salsa identities or deferred randomness.

use std::fmt::Write;

use anyhow::anyhow;
use anyhow::Context;
use oak_package_metadata::namespace::Namespace;
use oak_semantic::fuzz::Program;

use crate::file_imports::CollationView;
use crate::fuzz::spec::FileId;
use crate::fuzz::spec::Owner;
use crate::fuzz::spec::PackageId;
use crate::fuzz::spec::PackageKind;
use crate::fuzz::spec::PackageSpec;
use crate::fuzz::spec::WorkspaceSpec;
use crate::NamespaceVisibility;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Scenario {
    /// Identifies the originating corpus, not the mutated scenario.
    pub seed: u64,
    /// Position in the seed corpus.
    pub variant: usize,
    pub initial: WorkspaceSpec,
    /// Run first on a fresh database because Salsa's repeated key depends on
    /// entry order.
    pub cold_entry: Query,
    pub ops: Vec<Op>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum Op {
    Query(Query),
    Edit(Edit),
}

/// Use the same source override as `upsert_editor()` so edits do not bump the
/// file revision.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Edit {
    pub file: FileId,
    pub program: Program,
}

/// Production roots and direct entries to cycle-sensitive queries.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum Query {
    Diagnostics(FileId),
    Imports(FileId),
    ImportsAt(FileId, Site),
    ResolveAt(FileId, Site),
    Resolve(FileId, String),
    UsedPackages(FileId),
    SourcedBy(FileId),
    AllPackageDependencies,
    AllWorkspaceFileDependencies,
    AllWorkspaceLoaderDependencies,
    AllWorkspacePackageDependencies,
    DefaultSearchPathPackages,
    SemanticIndex(FileId),
    Exports(FileId),
    AttachedPackages(FileId),
    AttachedPackagesAnywhere(FileId),
    InheritedLayers(FileId, CollationView),
    CrossFileLayers(FileId, CollationView),
    PackageResolve(PackageId, String, NamespaceVisibility),
}

/// A semantic location for an offset-keyed query, resolved after each edit.
/// Stored byte offsets could point into unrelated replacement text.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub enum Site {
    /// Start of the first `source()` or `library()` callee, including nested calls.
    FirstCall,
    /// Start of the last identifier, which can be in deferred collation.
    LastIdentifier,
    /// One past the last byte, where the whole collation has loaded.
    Eof,
}

impl Scenario {
    pub(super) fn header(&self) -> String {
        format!("seed {} variant {}", self.seed, self.variant)
    }

    pub(super) fn render(&self) -> String {
        let mut out = String::new();
        let _ = write!(out, "{}", self.initial.render());
        let _ = writeln!(out, "  cold entry: {}", self.cold_entry.render());
        for (index, op) in self.ops.iter().enumerate() {
            let _ = writeln!(out, "  op {index}: {}", op.render());
        }
        out
    }

    /// Keep corpus entries and saved failures searchable as text.
    pub fn to_json(&self) -> anyhow::Result<String> {
        serde_json::to_string(self).context("failed to serialize scenario to JSON")
    }

    pub fn from_json(bytes: &[u8]) -> anyhow::Result<Scenario> {
        let scenario: Scenario =
            serde_json::from_slice(bytes).context("failed to parse scenario JSON")?;
        scenario.validate()?;
        Ok(scenario)
    }

    /// Rejects scenarios that would panic in `World` rather than exercise a query.
    pub fn validate(&self) -> anyhow::Result<()> {
        let file_count = self.initial.files.len();

        // `choose::random_file()` assumes a positive file count and
        // `mutate::add_file()` copies the first file's owner, so mutation can
        // neither target nor populate an empty workspace.
        if file_count == 0 {
            return Err(anyhow!("the workspace has no files"));
        }

        crate::fuzz::mutate::within_bounds(self)?;

        validate_file_id(self.cold_entry.file(), file_count, "cold_entry")?;
        for (index, op) in self.ops.iter().enumerate() {
            validate_file_id(op.file(), file_count, &format!("op {index}"))?;
        }

        let package_count = self.initial.packages.len();
        validate_package_id(self.cold_entry.package(), package_count, "cold_entry")?;
        for (index, op) in self.ops.iter().enumerate() {
            if let Op::Query(query) = op {
                validate_package_id(query.package(), package_count, &format!("op {index}"))?;
            }
        }

        for file in &self.initial.files {
            let Owner::Package(id) = file.owner else {
                continue;
            };
            let Some(package) = self.initial.packages.get(id.0) else {
                return Err(anyhow!(
                    "file {} owner references package {} but the workspace has {package_count} packages",
                    file.path, id.0
                ));
            };
            if package.kind != PackageKind::Workspace {
                return Err(anyhow!(
                    "file {} is owned by library package {}",
                    file.path,
                    package.name
                ));
            }
        }

        let mut names: Vec<&str> = self.initial.installed.iter().map(String::as_str).collect();
        for package in &self.initial.packages {
            if package.name == "base" {
                return Err(anyhow!("package cannot be named \"base\""));
            }
            if names.contains(&package.name.as_str()) {
                return Err(anyhow!(
                    "package name \"{}\" is used more than once",
                    package.name
                ));
            }
            names.push(&package.name);
            validate_namespace(package)?;
        }

        Ok(())
    }
}

/// Reject directives that fail to parse or lose names during parsing.
/// [`is_identifier()`] admits R reserved words such as `if`, which fail to
/// parse, and literals such as `TRUE`, which parse but yield no name.
///
/// A second `importFrom()` for a name is rejected rather than collapsed. The
/// parser keeps one of them, and the report would then describe an edge the
/// database does not have.
fn validate_namespace(package: &PackageSpec) -> anyhow::Result<()> {
    let mut reexported: Vec<&str> = Vec::new();
    for reexport in &package.reexports {
        if reexported.contains(&reexport.name.as_str()) {
            return Err(anyhow!(
                "package {} re-exports {} more than once",
                package.name,
                reexport.name
            ));
        }
        reexported.push(&reexport.name);
    }

    let namespace = match Namespace::parse(&package.namespace_text()) {
        Ok(namespace) => namespace,
        Err(err) => {
            return Err(anyhow!(
                "package {} renders a NAMESPACE that does not parse: {err}",
                package.name
            ))
        },
    };

    for export in &package.exports {
        if !namespace.exports.contains_str(export) {
            return Err(anyhow!(
                "package {} exports {export}, which the NAMESPACE parser reads as no name",
                package.name
            ));
        }
    }
    for reexport in &package.reexports {
        let read_back = namespace
            .imports
            .iter()
            .any(|import| import.name == reexport.name && import.package == reexport.from);
        if !read_back {
            return Err(anyhow!(
                "package {} imports {} from {}, which the NAMESPACE parser reads as no name",
                package.name,
                reexport.name,
                reexport.from
            ));
        }
    }

    Ok(())
}

/// `file` is `None` for the aggregate queries, which have no file to check.
fn validate_file_id(file: Option<FileId>, file_count: usize, context: &str) -> anyhow::Result<()> {
    let Some(file) = file else {
        return Ok(());
    };
    if file.0 >= file_count {
        return Err(anyhow!(
            "{context} references file {} but the workspace has {file_count} files",
            file.0
        ));
    }
    Ok(())
}

fn validate_package_id(
    package: Option<PackageId>,
    package_count: usize,
    context: &str,
) -> anyhow::Result<()> {
    let Some(package) = package else {
        return Ok(());
    };
    if package.0 >= package_count {
        return Err(anyhow!(
            "{context} references package {} but the workspace has {package_count} packages",
            package.0
        ));
    }
    Ok(())
}

impl Op {
    /// Expose an operation's file so removal mutations can retarget it.
    pub(super) fn file_mut(&mut self) -> Option<&mut FileId> {
        match self {
            Op::Query(query) => query.file_mut(),
            Op::Edit(edit) => Some(&mut edit.file),
        }
    }

    pub(super) fn file(&self) -> Option<FileId> {
        match self {
            Op::Query(query) => query.file(),
            Op::Edit(edit) => Some(edit.file),
        }
    }

    pub(super) fn render(&self) -> String {
        match self {
            Op::Query(query) => query.render(),
            Op::Edit(edit) => format!("edit [{}] -> {:?}", edit.file.0, edit.program.render().text),
        }
    }
}

impl Query {
    pub(super) fn file_mut(&mut self) -> Option<&mut FileId> {
        match self {
            Query::Diagnostics(file) |
            Query::Imports(file) |
            Query::ImportsAt(file, _) |
            Query::ResolveAt(file, _) |
            Query::Resolve(file, _) |
            Query::UsedPackages(file) |
            Query::SourcedBy(file) |
            Query::SemanticIndex(file) |
            Query::Exports(file) |
            Query::AttachedPackages(file) |
            Query::AttachedPackagesAnywhere(file) |
            Query::InheritedLayers(file, _) |
            Query::CrossFileLayers(file, _) => Some(file),
            Query::AllPackageDependencies |
            Query::AllWorkspaceFileDependencies |
            Query::AllWorkspaceLoaderDependencies |
            Query::AllWorkspacePackageDependencies |
            Query::DefaultSearchPathPackages |
            Query::PackageResolve(..) => None,
        }
    }

    pub(super) fn file(&self) -> Option<FileId> {
        match self {
            Query::Diagnostics(file) |
            Query::Imports(file) |
            Query::ImportsAt(file, _) |
            Query::ResolveAt(file, _) |
            Query::Resolve(file, _) |
            Query::UsedPackages(file) |
            Query::SourcedBy(file) |
            Query::SemanticIndex(file) |
            Query::Exports(file) |
            Query::AttachedPackages(file) |
            Query::AttachedPackagesAnywhere(file) |
            Query::InheritedLayers(file, _) |
            Query::CrossFileLayers(file, _) => Some(*file),
            Query::AllPackageDependencies |
            Query::AllWorkspaceFileDependencies |
            Query::AllWorkspaceLoaderDependencies |
            Query::AllWorkspacePackageDependencies |
            Query::DefaultSearchPathPackages |
            Query::PackageResolve(..) => None,
        }
    }

    pub(super) fn package(&self) -> Option<PackageId> {
        match self {
            Query::PackageResolve(package, _, _) => Some(*package),
            _ => None,
        }
    }

    pub(super) fn package_mut(&mut self) -> Option<&mut PackageId> {
        match self {
            Query::PackageResolve(package, _, _) => Some(package),
            _ => None,
        }
    }

    pub(super) fn render(&self) -> String {
        match self {
            Query::Diagnostics(file) => format!("diagnostics[{}]", file.0),
            Query::Imports(file) => format!("imports[{}]", file.0),
            Query::ImportsAt(file, site) => format!("imports_at[{}] {site:?}", file.0),
            Query::ResolveAt(file, site) => format!("resolve_at[{}] {site:?}", file.0),
            Query::Resolve(file, name) => format!("resolve[{}] {name}", file.0),
            Query::UsedPackages(file) => format!("used_packages[{}]", file.0),
            Query::SourcedBy(file) => format!("sourced_by[{}]", file.0),
            Query::AllPackageDependencies => "all_package_dependencies".to_string(),
            Query::AllWorkspaceFileDependencies => "all_workspace_file_dependencies".to_string(),
            Query::AllWorkspaceLoaderDependencies => {
                "all_workspace_loader_dependencies".to_string()
            },
            Query::AllWorkspacePackageDependencies => {
                "all_workspace_package_dependencies".to_string()
            },
            Query::DefaultSearchPathPackages => "default_search_path_packages".to_string(),
            Query::SemanticIndex(file) => format!("semantic_index[{}]", file.0),
            Query::Exports(file) => format!("exports[{}]", file.0),
            Query::AttachedPackages(file) => format!("attached_packages[{}]", file.0),
            Query::AttachedPackagesAnywhere(file) => {
                format!("attached_packages_anywhere[{}]", file.0)
            },
            Query::InheritedLayers(file, view) => {
                format!("inherited_layers[{}] {view:?}", file.0)
            },
            Query::CrossFileLayers(file, view) => {
                format!("cross_file_layers[{}] {view:?}", file.0)
            },
            Query::PackageResolve(package, name, visibility) => {
                format!("package_resolve[p{}] {name} {visibility:?}", package.0)
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use mutatis::Session;
    use oak_semantic::effects::fuzz::EffectRecipe;
    use oak_semantic::fuzz::Expr;
    use oak_semantic::fuzz::Invocation;
    use oak_semantic::fuzz::Stmt;

    use super::Edit;
    use super::FileId;
    use super::Op;
    use super::Owner;
    use super::PackageId;
    use super::Program;
    use super::Query;
    use super::Scenario;
    use super::WorkspaceSpec;
    use crate::fuzz::build::binding;
    use crate::fuzz::build::function_def;
    use crate::fuzz::build::source;
    use crate::fuzz::corpus;
    use crate::fuzz::corpus::corpus;
    use crate::fuzz::seed_corpus;
    use crate::fuzz::spec::Reexport;
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

    /// Legacy input decodes to the canonical model and serializes in that format.
    #[test]
    fn test_legacy_package_field_decodes_into_packages() {
        let scenario = corpus::case("recursive_source_dir_in_package");
        let json = scenario.to_json().unwrap();

        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let initial = value.get_mut("initial").unwrap().as_object_mut().unwrap();
        let packages = initial.remove("packages").unwrap();
        let name = packages[0]["name"].as_str().unwrap().to_string();
        initial.insert("package".to_string(), serde_json::Value::String(name));
        for file in initial["files"].as_array_mut().unwrap() {
            if file["owner"] == serde_json::json!({"Package": 0}) {
                file["owner"] = serde_json::Value::String("Package".to_string());
            }
        }

        let legacy = serde_json::to_string(&value).unwrap();
        let restored = Scenario::from_json(legacy.as_bytes()).unwrap();

        assert_eq!(restored.render(), scenario.render());
        assert_eq!(restored.to_json().unwrap(), json);
    }

    #[test]
    fn test_legacy_package_and_packages_together_is_rejected() {
        let scenario = corpus::case("recursive_source_dir_in_package");
        let json = scenario.to_json().unwrap();

        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let initial = value.get_mut("initial").unwrap().as_object_mut().unwrap();
        let name = initial["packages"][0]["name"].as_str().unwrap().to_string();
        initial.insert("package".to_string(), serde_json::Value::String(name));

        let ambiguous = serde_json::to_string(&value).unwrap();
        assert!(Scenario::from_json(ambiguous.as_bytes()).is_err());
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
            "the workspace has 6 files but mutation produces at most 5"
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
            "op 2 nests 4 levels but mutation produces at most 3"
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
            "the workspace has 5 packages but mutation produces at most 3"
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
            "package \"pkgb\" has 4 exports but mutation produces at most 3"
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
            "package \"pkga\" has 4 reexports but mutation produces at most 3"
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
}
