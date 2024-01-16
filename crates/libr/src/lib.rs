//
// lib.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

mod constant_globals;
mod functions;
#[cfg(target_family = "windows")]
#[path = "graphapp.rs"]
mod graphapp_impl;
mod mutable_globals;
mod r;
mod sys;

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
        pub use crate::graphapp_impl::functions_initializer::functions;
    }

    pub mod has {
        pub use crate::graphapp_impl::functions_has::*;
    }

    pub use crate::graphapp_impl::*;
}
