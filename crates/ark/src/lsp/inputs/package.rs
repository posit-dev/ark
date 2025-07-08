//
// package.rs
//
// Copyright (C) 2025 by Posit Software, PBC
//
//

use std::path::PathBuf;

/// Represents an R package and its metadata relevant for static analysis.
#[derive(Clone, Debug)]
pub struct Package {
    pub name: String,
    pub version: String,

    /// Path to the directory that contains `DESCRIPTION`. Could be an installed
    /// package, or a package source.
    pub path: PathBuf,

    /// Imports and exports in `NAMESPACE`
    pub imports: Vec<String>,
    pub exports: Vec<String>,

    /// `Depends` field in `DESCRIPTION`
    pub depends: Vec<String>,
}
