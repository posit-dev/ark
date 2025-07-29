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
}

impl Package {
    pub fn new(path: PathBuf, description: Description, namespace: Namespace) -> Self {
        Self {
            path,
            description,
            namespace,
            documentation: Default::default(),
        }
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

        Ok(Some(Package {
            path: package_path.to_path_buf(),
            description,
            namespace,
            documentation,
        }))
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
