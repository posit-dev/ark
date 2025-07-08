//
// source_root.rs
//
// Copyright (C) 2025 by Posit Software, PBC
//
//

use super::package::Package;

/// The root of a source tree.
/// Currently only supports packages, but can be extended to scripts.
#[derive(Clone, Debug)]
pub enum SourceRoot {
    Package(Package),
    // Scripts(Vec<Script>),   // For reference, to implement later on
}
