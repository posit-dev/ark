//
// mutable_globals.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

macro_rules! generate {
    (
        $(
            $(#[doc=$doc:expr])*
            $(#[cfg($cfg:meta)])*
            pub static mut $name:ident: $ty:ty;
        )+
    ) => (
        // Define internal global pointers, initialized to null pointer
        $(
            $(#[cfg($cfg)])*
            static mut $name: *mut $ty = std::ptr::null_mut();
        )+

        // Define getter and setter pairs
        $(
            paste::paste! {
                $(#[doc=$doc])*
                $(#[cfg($cfg)])*
                pub unsafe fn [<$name _get>]() -> $ty {
                    *$name
                }

                $(#[doc=$doc])*
                $(#[cfg($cfg)])*
                pub unsafe fn [<$name _set>](x: $ty) {
                    *$name = x;
                }
            }
        )+

        // Make `has::` helpers for each global.
        // i.e. `libr::has::Rf_error()`.
        pub(super) mod mutable_globals_has {
            $(
                paste::paste! {
                    $(#[doc=$doc])*
                    $(#[cfg($cfg)])*
                    pub unsafe fn $name() -> bool {
                        super::$name.is_null()
                    }
                }
            )+
        }

        pub(super) mod mutable_globals_initializer {
            use super::*;

            /// Initialize library mutable globals
            pub fn mutable_globals(library: &libloading::Library) {
                $(
                    $(#[cfg($cfg)])*
                    paste::paste! {
                        let symbol = unsafe { library.get(stringify!($name).as_bytes()) };

                        // If the symbol doesn't exist in the library, assume it simply
                        // isn't available in this version of R.
                        if let Ok(symbol) = symbol {
                            // Pull into Rust as a mutable pointer
                            unsafe { $name = *symbol };
                        }
                    }
                )+
            }
        }
    );
}

pub(crate) use generate;
