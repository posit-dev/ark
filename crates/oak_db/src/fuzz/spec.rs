//! Keeps failure reports independent of Salsa identities.

use std::fmt::Write;

use anyhow::anyhow;
use oak_semantic::fuzz::Program;

pub(super) const SCRIPT_ROOT: &str = "w";

pub(super) const LIBRARY_ROOT: &str = "libs";

/// Parent of every workspace package's own root.
pub(super) const PACKAGE_ROOT: &str = "p";

/// Index into [`WorkspaceSpec::files`]. Stable across edits so operations keep
/// naming the same file.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct FileId(pub usize);

/// Index into [`WorkspaceSpec::packages`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct PackageId(pub usize);

/// Determines the root against which relative `source()` paths resolve.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize)]
pub enum Owner {
    /// A loose script under the scripts workspace root.
    Script,
    /// An `R/` file of a workspace package, under that package's own root.
    Package(PackageId),
}

/// Maps the legacy `"Package"` owner to slot 0, where the workspace decoder
/// places the single package from that format.
impl<'de> serde::Deserialize<'de> for Owner {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Tag(String),
            Package {
                #[serde(rename = "Package")]
                id: usize,
            },
        }

        match Repr::deserialize(deserializer)? {
            Repr::Tag(tag) if tag == "Script" => Ok(Owner::Script),
            Repr::Tag(tag) if tag == "Package" => Ok(Owner::Package(PackageId(0))),
            Repr::Tag(tag) => Err(serde::de::Error::custom(format!(
                "unknown Owner tag {tag:?}"
            ))),
            Repr::Package { id } => Ok(Owner::Package(PackageId(id))),
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FileSpec {
    pub owner: Owner,
    /// Relative to the owning root because `source()` resolves from `anchor_dir()`.
    pub path: String,
    pub program: Program,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum PackageKind {
    /// Has its own root at `p/{name}` and can own files.
    Workspace,
    /// Sits in the library root with metadata but no files.
    Library,
}

/// One `importFrom(from, name)` directive.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Reexport {
    pub name: String,
    /// Keeps the import dangling when its source package is removed.
    pub from: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PackageSpec {
    pub name: String,
    pub kind: PackageKind,
    /// Rendered as `export()` directives.
    pub exports: Vec<String>,
    /// Rendered as `importFrom()` directives.
    pub reexports: Vec<Reexport>,
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
#[serde(try_from = "WorkspaceSpecRepr")]
pub struct WorkspaceSpec {
    /// Includes `base` when `source()` or `library()` needs it to resolve.
    pub installed: Vec<String>,
    pub packages: Vec<PackageSpec>,
    /// Indexed by [`FileId`]. Order is script order or package collation order.
    pub files: Vec<FileSpec>,
}

/// Decodes both the current shape and the pre-multi-package shape, so a
/// scenario saved before `packages` existed keeps replaying.
#[derive(serde::Deserialize)]
struct WorkspaceSpecRepr {
    installed: Vec<String>,
    files: Vec<FileSpec>,
    #[serde(default)]
    packages: Vec<PackageSpec>,
    #[serde(default)]
    package: Option<String>,
}

impl TryFrom<WorkspaceSpecRepr> for WorkspaceSpec {
    type Error = anyhow::Error;

    fn try_from(repr: WorkspaceSpecRepr) -> anyhow::Result<Self> {
        let packages = match (repr.package, repr.packages.is_empty()) {
            (Some(_), false) => {
                return Err(anyhow!(
                    "scenario carries both the legacy `package` field and `packages`"
                ))
            },
            (Some(name), true) => vec![PackageSpec {
                name,
                kind: PackageKind::Workspace,
                exports: Vec::new(),
                reexports: Vec::new(),
            }],
            (None, _) => repr.packages,
        };
        Ok(WorkspaceSpec {
            installed: repr.installed,
            packages,
            files: repr.files,
        })
    }
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
