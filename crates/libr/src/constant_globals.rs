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
            #[default=$default:expr]
            pub static $name:ident: $ty:ty;
        )+
    ) => (
        // Define globals
        // The macro signature doesn't use `mut` so that we convey that they are meant
        // to be constant, but really they have to be mutable because we need to define
        // them to their initial value
        $(
            $(#[doc=$doc])*
            $(#[cfg($cfg)])*
            pub static mut $name: $ty = $default;
        )+

        // Define `has::` booleans
        $(
            paste::paste! {
                $(#[cfg($cfg)])*
                static mut [<$name _has>]: bool = false;
            }
        )+

        // Make `has::` helpers for each global.
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
            pub fn constant_globals(library: &libloading::Library) {
                $(
                    $(#[cfg($cfg)])*
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
