//
// constant_globals.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

macro_rules! generate {
    (
        $(
            $(#[doc=$doc:expr])*
            $(#[cfg($cfg:meta)])*
            pub static mut $name:ident: $ty:ty = $default:expr;
        )+
    ) => (
        // Define globals, initialized to a default value
        // (otherwise we can't make them static mut)
        $(
            $(#[doc=$doc])*
            $(#[cfg($cfg)])*
            pub static mut $name: $ty = $default;
        )+

        // Define `has::` booleans
        $(
            paste::paste! {
                static mut [<$name _has>]: bool = false;
            }
        )+

        // Make `has::` helpers for each function.
        // i.e. `libr::has::Rf_error()`.
        mod constant_globals_has {
            use super::*;

            $(
                paste::paste! {
                    $(#[doc=$doc])*
                    $(#[cfg($cfg)])*
                    pub unsafe fn $name() -> bool {
                        [<$name _has>]
                    }
                }
            )+
        }

        mod constant_globals_initializer {
            use super::*;

            /// Initialize library constant globals
            pub fn initialize(library: &libloading::Library) {
                $(
                    paste::paste! {
                        let symbol = unsafe { library.get(stringify!($name).as_bytes()) };

                        // If the symbol doesn't exist in the library, assume it simply
                        // isn't available in this version of R.
                        if let Ok(symbol) = symbol {
                            // Pull into Rust as a constant pointer
                            let pointer: *const $ty = *symbol;

                            // Assume global has been initialized on the R side, and is
                            // otherwise constant, so deref to copy it over and assign it
                            // once.
                            unsafe { $name = *pointer };

                            // Update our `has::` marker
                            unsafe { [<$name _has>] = true };
                        }
                    }
                )+
            }
        }
    );
}

pub(crate) use generate;
