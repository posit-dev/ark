//
// lib.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

mod constant_globals;
mod functions;
mod functions_variadic;
mod mutable_globals;
mod r;
mod sys;
mod types;
#[cfg(target_family = "windows")]
#[path = "graphapp.rs"]
mod windows_graphapp;

// ---------------------------------------------------------------------------------------
// R

/// Initialization functions that must be called before using any functions or globals
/// exported by the crate
pub mod initialize {
    pub use crate::r::constant_globals_initializer::constant_globals;
    pub use crate::r::functions_initializer::functions;
    pub use crate::r::functions_variadic_initializer::functions_variadic;
    pub use crate::r::mutable_globals_initializer::mutable_globals;
}

pub mod has {
    pub use crate::r::constant_globals_has::*;
    pub use crate::r::functions_has::*;
    pub use crate::r::functions_variadic_has::*;
    pub use crate::r::mutable_globals_has::*;
}

// Expose all R types, API functions, and API globals at the top level
pub use r::*;
pub use types::*;

// ---------------------------------------------------------------------------------------
// graphapp

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
