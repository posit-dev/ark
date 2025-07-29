//
// package.rs
//
// Copyright (C) 2025 by Posit Software, PBC
//
//

use std::fs;
use std::path::PathBuf;

use crate::lsp::inputs::documentation::Documentation;
use crate::lsp::inputs::package_description::Description;
use crate::lsp::inputs::package_namespace::Namespace;

/// Represents an R package and its metadata relevant for static analysis.
#[derive(Clone, Debug)]
pub struct Package {
    /// Path to the directory that contains `DESCRIPTION``. Can
    /// be an installed package or a package source.
    pub path: PathBuf,

    pub description: Description,
    pub namespace: Namespace,
    pub documentation: Documentation,

    // List of symbols exported via NAMESPACE `export()` directives and via
    // `DocType{data}`. Note the latter should only apply to packages with
    // `LazyData: true` but currently applies to all packages, as a stopgap
    // to prevent spurious diagnostics (we accept false negatives to avoid
    // annoying false positives).
    pub exported_symbols: Vec<String>,
}

impl Package {
    pub fn new(
        path: PathBuf,
        description: Description,
        namespace: Namespace,
        documentation: Documentation,
    ) -> Self {
        // Compute exported symbols. Start from explicit NAMESPACE exports.
        let mut exported_symbols = namespace.exports.clone();

        // Add exported datasets. Ideally we'd only do that for packages
        // specifying `LazyData: true`.
        let exported_datasets = documentation.rd_files.iter().filter_map(|rd| {
            if rd.doc_type == Some(crate::lsp::inputs::documentation_rd_file::RdDocType::Data) {
                rd.name.clone()
            } else {
                None
            }
        });
        exported_symbols.extend(exported_datasets);

        Self {
            path,
            description,
            namespace,
            documentation,
            exported_symbols,
        }
    }

    #[cfg(test)]
    pub fn from_parts(path: PathBuf, description: Description, namespace: Namespace) -> Self {
        Self::new(path, description, namespace, Default::default())
    }

    /// Load a package from a given path.
    pub fn load_from_folder(package_path: &std::path::Path) -> anyhow::Result<Option<Self>> {
        let description_path = package_path.join("DESCRIPTION");

        // Only consider directories that contain a description file
        if !description_path.is_file() {
            return Ok(None);
        }

        // This fails if there is no `Package` field, so we're never loading
        // folders like bookdown projects as package
        let description_contents = fs::read_to_string(&description_path)?;
        let description = Description::parse(&description_contents)?;

        let namespace_path = package_path.join("NAMESPACE");
        let namespace = if namespace_path.is_file() {
            let namespace_contents = fs::read_to_string(&namespace_path)?;
            Namespace::parse(&namespace_contents)?
        } else {
            tracing::info!(
                "Package `{name}` doesn't contain a NAMESPACE file, using defaults",
                name = description.name
            );
            Namespace::default()
        };

        let documentation_path = package_path.join("man");
        let documentation = match Documentation::load_from_folder(&documentation_path) {
            Ok(documentation) => documentation,
            Err(err) => {
                tracing::warn!("Can't load package documentation: {err:?}");
                Documentation::default()
            },
        };

        Ok(Some(Self::new(
            package_path.to_path_buf(),
            description,
            namespace,
            documentation,
        )))
    }

    /// Load a package from the given library path and name.
    pub fn load_from_library(
        lib_path: &std::path::Path,
        name: &str,
    ) -> anyhow::Result<Option<Self>> {
        let package_path = lib_path.join(name);

        // For library packages, ensure the invariant that the package name
        // matches the folder name
        if let Some(pkg) = Self::load_from_folder(&package_path)? {
            if pkg.description.name != name {
                return Err(anyhow::anyhow!(
                    "`Package` field in `DESCRIPTION` doesn't match folder name '{name}'"
                ));
            }
            Ok(Some(pkg))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::inputs::documentation_rd_file::RdDocType;
    use crate::lsp::inputs::documentation_rd_file::RdFile;
    use crate::lsp::inputs::package_description::Description;
    use crate::lsp::inputs::package_namespace::Namespace;

    #[test]
    fn test_exported_symbols_combining_namespace_and_rd_files() {
        let namespace = Namespace {
            exports: vec!["foo".to_string(), "bar".to_string()],
            ..Default::default()
        };

        let rd_files = vec![
            RdFile {
                name: Some("data1".to_string()),
                doc_type: Some(RdDocType::Data),
            },
            RdFile {
                name: Some("pkgdoc".to_string()),
                doc_type: Some(RdDocType::Package),
            },
            RdFile {
                name: Some("other".to_string()),
                doc_type: None,
            },
        ];
        let documentation = Documentation { rd_files };

        let description = Description {
            name: "mypkg".to_string(),
            version: "1.0.0".to_string(),
            depends: vec![],
            fields: Default::default(),
        };

        let package = Package::new(
            PathBuf::from("/mock/path"),
            description,
            namespace,
            documentation,
        );

        assert!(package.exported_symbols.contains(&"foo".to_string()));
        assert!(package.exported_symbols.contains(&"bar".to_string()));
        assert!(package.exported_symbols.contains(&"data1".to_string()));

        assert!(!package.exported_symbols.contains(&"pkgdoc".to_string()));
        assert!(!package.exported_symbols.contains(&"other".to_string()));
    }
}
