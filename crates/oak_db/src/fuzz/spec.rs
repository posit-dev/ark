//! Keeps failure reports independent of Salsa identities.

use std::fmt::Write;

use oak_semantic::fuzz::Program;

pub(super) const SCRIPT_ROOT: &str = "w";

pub(super) const LIBRARY_ROOT: &str = "libs";

/// Index into [`WorkspaceSpec::files`]. Stable across edits so operations keep
/// naming the same file.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct FileId(pub usize);

/// Determines the root against which relative `source()` paths resolve.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Owner {
    /// A loose script under the scripts workspace root.
    Script,
    /// An `R/` file of the workspace package, under its own root.
    Package,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FileSpec {
    pub owner: Owner,
    /// Relative to the owning root because `source()` resolves from `anchor_dir()`.
    pub path: String,
    pub program: Program,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceSpec {
    /// Includes `base` when `source()` or `library()` needs it to resolve.
    pub installed: Vec<String>,
    pub package: Option<String>,
    /// Indexed by [`FileId`]. Order is script order or package collation order.
    pub files: Vec<FileSpec>,
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

    pub(super) fn package_root(&self) -> Option<String> {
        self.package.as_ref().map(|name| format!("p/{name}"))
    }

    pub(super) fn root_path(&self, owner: Owner) -> String {
        match owner {
            Owner::Script => SCRIPT_ROOT.to_string(),
            Owner::Package => match self.package_root() {
                Some(root) => root,
                None => panic!("package-owned file in a spec with no package"),
            },
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
        if let Some(name) = &self.package {
            let _ = writeln!(out, "  package: {name}");
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
