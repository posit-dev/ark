//
// package.rs
//
// Copyright (C) 2025 by Posit Software, PBC
//
//

use std::fs;
use std::path::PathBuf;

use crate::lsp::inputs::package_description::Description;
use crate::lsp::inputs::package_namespace::Namespace;

/// Represents an R package and its metadata relevant for static analysis.
#[derive(Clone, Debug)]
pub struct Package {
    /// Path to the directory that contains `DESCRIPTION` and `NAMESPACE`. Can
    /// be an installed package or a package source.
    pub path: PathBuf,

    pub description: Description,
    pub namespace: Namespace,
}

impl Package {
    /// Attempts to load a package from the given path and name.
    pub fn load(lib_path: &std::path::Path, name: &str) -> anyhow::Result<Option<Self>> {
        let package_path = lib_path.join(name);

        let description_path = package_path.join("DESCRIPTION");
        let namespace_path = package_path.join("NAMESPACE");

        // Only consider libraries that have a folder named after the
        // requested package and that contains a description file
        if !description_path.is_file() {
            return Ok(None);
        }

        // This fails if there is no `Package` field, so we're never loading
        // folders like bookdown projects as package
        let description_contents = fs::read_to_string(&description_path)?;
        let description = Description::parse(&description_contents)?;

        if description.name != name {
            return Err(anyhow::anyhow!(
                "`Package` field in `DESCRIPTION` doesn't match folder name '{name}'"
            ));
        }

        let namespace_contents = fs::read_to_string(&namespace_path)?;
        let namespace = Namespace::parse(&namespace_contents)?;

        Ok(Some(Package {
            path: package_path.to_path_buf(),
            description,
            namespace,
        }))
    }
}
