//! Keeps failure reports independent of Salsa identities.

use std::fmt::Write;

use oak_semantic::fuzz::Program;

use crate::classify_in_package;
use crate::PackagePlacement;

pub(super) const SCRIPT_ROOT: &str = "w";

pub(super) const LIBRARY_ROOT: &str = "libs";

/// Parent of every workspace package's own root.
pub(super) const PACKAGE_ROOT: &str = "p";

/// Index into [`WorkspaceSpec::files`]. Stable across edits so operations keep
/// naming the same file.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct FileId(pub(crate) usize);

/// Index into [`WorkspaceSpec::packages`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct PackageId(pub(crate) usize);

/// Determines the root against which relative `source()` paths resolve.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum Owner {
    /// A loose script under the scripts workspace root.
    Script,
    /// An `R/` file of a workspace package, under that package's own root.
    Package(PackageId),
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct FileSpec {
    pub(crate) owner: Owner,
    /// Relative to the owning root because `source()` resolves from `anchor_dir()`.
    pub(crate) path: String,
    pub(crate) program: Program,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum PackageKind {
    /// Has its own root at `p/{name}` and can own files.
    Workspace,
    /// Sits in the library root with metadata but no files.
    Library,
}

/// One `importFrom(from, name)` directive.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Reexport {
    pub(crate) name: String,
    /// Keeps the import dangling when its source package is removed.
    pub(crate) from: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct PackageSpec {
    pub(crate) name: String,
    pub(crate) kind: PackageKind,
    /// Rendered as `export()` directives.
    pub(crate) exports: Vec<String>,
    /// Rendered as `importFrom()` directives.
    pub(crate) reexports: Vec<Reexport>,
}

impl PackageSpec {
    /// Directory holding the package's `DESCRIPTION` and `NAMESPACE`. A
    /// `Workspace` package's directory is also its root. A `Library`
    /// package's sits inside the shared library root.
    pub(super) fn directory(&self) -> String {
        match self.kind {
            PackageKind::Workspace => format!("{PACKAGE_ROOT}/{}", self.name),
            PackageKind::Library => format!("{LIBRARY_ROOT}/{}", self.name),
        }
    }

    /// Shared by reporting and materialization so both use the same directives.
    pub(super) fn namespace_text(&self) -> String {
        let mut out = String::new();
        for export in &self.exports {
            let _ = writeln!(out, "export({export})");
        }
        for reexport in &self.reexports {
            let _ = writeln!(out, "importFrom({}, {})", reexport.from, reexport.name);
        }
        out
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct WorkspaceSpec {
    /// Includes `base` when `source()` or `library()` needs it to resolve.
    pub(crate) installed: Vec<String>,
    pub(crate) packages: Vec<PackageSpec>,
    /// Indexed by [`FileId`]. Order is script order or package collation order.
    pub(crate) files: Vec<FileSpec>,
}

/// Uses `classify_in_package()` so the fuzz model assigns package files to the
/// same collation as the scanner.
pub(super) fn is_collation_member(relative: &str) -> bool {
    let root = std::path::Path::new(PACKAGE_ROOT);
    classify_in_package(root, &root.join(relative)) == PackagePlacement::File
}

/// Restricts the name's spelling, but does not exclude R keywords or literals.
/// Scenario validation also checks that the NAMESPACE parser retains each name.
pub(super) fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic() && chars.all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
}

impl WorkspaceSpec {
    pub(super) fn file(&self, id: FileId) -> &FileSpec {
        &self.files[id.0]
    }

    pub(super) fn file_mut(&mut self, id: FileId) -> &mut FileSpec {
        &mut self.files[id.0]
    }

    pub(super) fn ids(&self) -> impl Iterator<Item = FileId> {
        (0..self.files.len()).map(FileId)
    }

    pub(super) fn root_path(&self, owner: Owner) -> String {
        match owner {
            Owner::Script => SCRIPT_ROOT.to_string(),
            Owner::Package(id) => self.packages[id.0].directory(),
        }
    }

    pub(super) fn absolute_path(&self, id: FileId) -> String {
        let file = self.file(id);
        format!("{}/{}", self.root_path(file.owner), file.path)
    }

    /// Indents file bodies to distinguish R code from report metadata.
    pub(super) fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "  installed: {}", self.installed.join(", "));
        for (index, package) in self.packages.iter().enumerate() {
            let _ = writeln!(out, "  [p{index}] {} ({:?})", package.name, package.kind);
            for line in package.namespace_text().lines() {
                let _ = writeln!(out, "      | {line}");
            }
        }
        for id in self.ids() {
            let _ = writeln!(out, "  [{}] {}", id.0, self.absolute_path(id));
            for line in self.file(id).program.render().text.lines() {
                let _ = writeln!(out, "      | {line}");
            }
        }
        out
    }
}
