//! Stores concrete scenarios without Salsa identities or deferred randomness.

use std::fmt::Write;

use oak_semantic::fuzz::Program;

use crate::file_imports::CollationView;
use crate::tests::fuzz::spec::FileId;
use crate::tests::fuzz::spec::WorkspaceSpec;

#[derive(Clone, Debug)]
pub(super) struct Scenario {
    pub(super) seed: u64,
    /// Distinguishes cold entries sharing one workspace and history.
    pub(super) variant: usize,
    pub(super) initial: WorkspaceSpec,
    /// Run first on a fresh database because Salsa's repeated key depends on
    /// entry order.
    pub(super) cold_entry: Query,
    pub(super) ops: Vec<Op>,
}

#[derive(Clone, Debug)]
pub(super) enum Op {
    Query(Query),
    Edit(Edit),
}

/// Use the same source override as `upsert_editor()` so edits do not bump the
/// file revision.
#[derive(Clone, Debug)]
pub(super) struct Edit {
    pub(super) file: FileId,
    pub(super) program: Program,
}

/// Production roots and direct entries to cycle-sensitive queries.
#[derive(Clone, Debug)]
pub(super) enum Query {
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
#[derive(Clone, Copy, Debug)]
pub(super) enum Site {
    /// Start of the first `source()` or `library()` callee, including nested calls.
    FirstCall,
    /// Start of the last identifier, which can be in deferred collation.
    LastIdentifier,
    /// One past the last byte, where the whole collation has loaded.
    Eof,
}

impl Scenario {
    /// Identify the scenario before execution because hangs and aborts do not
    /// reach the unwind report.
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
}

impl Op {
    pub(super) fn render(&self) -> String {
        match self {
            Op::Query(query) => query.render(),
            Op::Edit(edit) => format!("edit [{}] -> {:?}", edit.file.0, edit.program.render().text),
        }
    }
}

impl Query {
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
