//! Stores concrete scenarios without Salsa identities or deferred randomness.
//!
//! Reference and namespace checks live in `validate`; serialization and
//! compatibility regression tests live in `tests`.

mod validate;

#[cfg(test)]
mod tests;

use std::fmt::Write;

use anyhow::Context;
use oak_semantic::fuzz::Program;

use crate::file_imports::CollationView;
use crate::fuzz::spec::FileId;
use crate::fuzz::spec::PackageId;
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
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
