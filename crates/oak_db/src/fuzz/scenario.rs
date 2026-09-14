//! Stores concrete scenarios without Salsa identities or deferred randomness.

use std::fmt::Write;

use anyhow::anyhow;
use anyhow::Context;
use oak_semantic::fuzz::Program;

use crate::file_imports::CollationView;
use crate::fuzz::spec::FileId;
use crate::fuzz::spec::Owner;
use crate::fuzz::spec::WorkspaceSpec;

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

        validate_file_id(self.cold_entry.file(), file_count, "cold_entry")?;
        for (index, op) in self.ops.iter().enumerate() {
            validate_file_id(op.file(), file_count, &format!("op {index}"))?;
        }

        let package_owned = self
            .initial
            .files
            .iter()
            .find(|file| file.owner == Owner::Package);
        if let (Some(file), None) = (package_owned, &self.initial.package) {
            return Err(anyhow!(
                "file {} is package-owned but the workspace declares no package",
                file.path
            ));
        }

        Ok(())
    }
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
            Query::DefaultSearchPathPackages => None,
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
            Query::DefaultSearchPathPackages => None,
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

    use super::FileId;
    use super::Op;
    use super::Owner;
    use super::Program;
    use super::Query;
    use super::Scenario;
    use super::WorkspaceSpec;
    use crate::fuzz::build::source;
    use crate::fuzz::corpus;
    use crate::fuzz::corpus::corpus;
    use crate::fuzz::seed_corpus;
    use crate::fuzz::ScenarioMutator;

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
        let scenario = Scenario {
            seed: 0,
            variant: 0,
            initial: WorkspaceSpec {
                installed: vec![],
                package: None,
                files: vec![],
            },
            cold_entry: Query::Diagnostics(FileId(0)),
            ops: vec![],
        };

        let json = scenario.to_json().unwrap();
        assert!(Scenario::from_json(json.as_bytes()).is_err());
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
    fn test_package_owned_file_without_package_is_rejected() {
        let mut scenario = corpus::case("acyclic_pair_closes_then_reopens");
        scenario.initial.files[0].owner = Owner::Package;

        let error = scenario.validate().unwrap_err();
        assert_eq!(
            error.to_string(),
            "file a.R is package-owned but the workspace declares no package"
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
}
