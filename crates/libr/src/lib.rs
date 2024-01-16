//
// lib.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

mod constant_globals;
mod functions;
mod mutable_globals;
mod r;
mod sys;
#[cfg(target_family = "windows")]
#[path = "graphapp.rs"]
mod windows_graphapp;

// ---------------------------------------------------------------------------------------

/// Initialization functions that must be called before using any functions or globals
/// exported by the crate
pub mod initialize {
    pub use crate::r::constant_globals_initializer::constant_globals;
    pub use crate::r::functions_initializer::functions;
    pub use crate::r::mutable_globals_initializer::mutable_globals;
}

pub mod has {
    pub use crate::r::constant_globals_has::*;
    pub use crate::r::functions_has::*;
    pub use crate::r::mutable_globals_has::*;
}

// Expose all of the R API at the top level
pub use crate::r::*;

#[cfg(target_family = "windows")]
pub mod graphapp {
    /// Initialization functions that must be called before using any functions or globals
    /// exported by the crate
    pub mod initialize {
        pub use crate::windows_graphapp::functions_initializer::functions;
    }

    pub mod has {
        pub use crate::windows_graphapp::functions_has::*;
    }

    pub use crate::windows_graphapp::*;
}
