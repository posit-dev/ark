//! Keeps failure reports independent of Salsa identities so a workspace can
//! be reconstructed from the reported data.

use std::fmt::Write;

pub(super) const SCRIPT_ROOT: &str = "w";

pub(super) const LIBRARY_ROOT: &str = "libs";

/// Index into [`WorkspaceSpec::files`]. Stable across edits, so an operation
/// recorded before execution keeps naming the same file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct FileId(pub(super) usize);

/// Which root a file belongs to, and therefore which relative paths a
/// `source()` call in it can name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Owner {
    /// A loose script under the scripts workspace root.
    Script,
    /// An `R/` file of the workspace package, under its own root.
    Package,
}

#[derive(Clone, Debug)]
pub(super) struct FileSpec {
    pub(super) owner: Owner,
    /// Store paths relative to the owning root because `source()` resolves relative
    /// arguments from that root through `anchor_dir()`.
    pub(super) path: String,
    pub(super) contents: String,
}

#[derive(Clone, Debug)]
pub(super) struct WorkspaceSpec {
    /// Include `base` so `source()` and `library()` can form the intended edges.
    pub(super) installed: Vec<String>,
    pub(super) package: Option<String>,
    /// Indexed by [`FileId`]. Order within an owner is the root's script order
    /// and the package's collation order.
    pub(super) files: Vec<FileSpec>,
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

    /// Indent file bodies so generated R code remains distinct from report
    /// metadata.
    pub(super) fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "  installed: {}", self.installed.join(", "));
        if let Some(name) = &self.package {
            let _ = writeln!(out, "  package: {name}");
        }
        for id in self.ids() {
            let _ = writeln!(out, "  [{}] {}", id.0, self.absolute_path(id));
            for line in self.file(id).contents.lines() {
                let _ = writeln!(out, "      | {line}");
            }
        }
        out
    }
}
