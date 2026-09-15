//! Validates scenario references, ownership, and namespace directives.

use anyhow::anyhow;
use oak_package_metadata::namespace::Namespace;

use super::Op;
use super::Scenario;
use crate::fuzz::spec::FileId;
use crate::fuzz::spec::Owner;
use crate::fuzz::spec::PackageId;
use crate::fuzz::spec::PackageKind;
use crate::fuzz::spec::PackageSpec;

impl Scenario {
    /// Rejects scenarios that would panic in `World` rather than exercise a query.
    pub fn validate(&self) -> anyhow::Result<()> {
        let file_count = self.initial.files.len();

        // Query generation assumes at least one file to target.
        if file_count == 0 {
            return Err(anyhow!("the workspace has no files"));
        }

        crate::fuzz::limits::within_bounds(self)?;

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
/// [`crate::fuzz::spec::is_identifier()`] admits R reserved words such as `if`, which fail to
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
