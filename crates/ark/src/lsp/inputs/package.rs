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
    /// Path to the directory that contains `DESCRIPTION`. Could be an installed
    /// package, or a package source.
    pub path: PathBuf,

    pub description: Description,
    pub namespace: Namespace,
}

/// Parsed DESCRIPTION file
#[derive(Clone, Debug)]
pub struct Description {
    pub name: String,
    pub version: String,

    /// `Depends` field
    pub depends: Vec<String>,
}

/// Parsed NAMESPACE file
#[derive(Clone, Debug)]
pub struct Namespace {
    pub imports: Vec<String>,
    pub exports: Vec<String>,
}
