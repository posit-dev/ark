//
// lib.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

mod constant_globals;
mod functions;
mod functions_variadic;
mod graphics;
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
pub use graphics::*;
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

// ---------------------------------------------------------------------------------------
// Helpers

/// Get the value of a mutable global using its pointer
///
/// We prefer using this over dereferencing the pointers directly.
///
/// NOTE: Must be `Copy`, as you can't move out of a raw pointer, so every access to
/// the pointer value generates a copy, but it should be cheap.
pub unsafe fn get<T>(x: *mut T) -> T
where
    T: Copy,
{
    *x
}

/// Set the value of a mutable global using its pointer
pub unsafe fn set<T>(x: *mut T, value: T) {
    *x = value;
}
