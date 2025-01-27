//
// functions.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

macro_rules! generate {
    (
        $(
            $(#[doc=$doc:expr])*
            $(#[cfg($cfg:meta)])*
            pub fn $name:ident($($pname:ident: $pty:ty), *) $(-> $ret:ty)*;
        )+
    ) => {
        // Make `Option` wrappers around the function pointers, initialized to `None`
        $(
            paste::paste! {
                $(#[cfg($cfg)])*
                static mut [<$name _opt>]: Option<unsafe extern "C" fn ($($pty), *) $(-> $ret)*> = None;
            }
        )+

        // Make public functions that pass through to the `Option` wrappers.
        // Initialization routine MUST be called before using these.
        $(
            paste::paste! {
                $(#[doc=$doc])*
                $(#[cfg($cfg)])*
                pub unsafe fn $name($($pname: $pty), *) $(-> $ret)* {
                    [<$name _opt>].unwrap_unchecked()($($pname), *)
                }
            }
        )+

        // Make `has::` helpers for each function.
        // i.e. `libr::has::Rf_error()`.
        pub(super) mod functions_has {
            use super::*;

            $(
                paste::paste! {
                    $(#[doc=$doc])*
                    $(#[cfg($cfg)])*
                    pub unsafe fn $name() -> bool {
                        matches!([<$name _opt>], Some(_))
                    }
                }
            )+
        }

        pub(super) mod functions_initializer {
            use super::*;

            /// Initialize library functions
            ///
            /// If we can't find it in the library, the `Option` wrapper remains `None`.
            /// This indicates that this version of R doesn't have that function.
            pub fn functions(library: &libloading::Library) {
                $(
                    $(#[cfg($cfg)])*
                    paste::paste! {
                        let symbol = unsafe { library.get(stringify!($name).as_bytes()) };

                        let pointer = match symbol {
                            Ok(symbol) => Some(*symbol),
                            Err(_) => None
                        };

                        unsafe { [<$name _opt>] = pointer };
                    }
                )+
            }
        }
    }
}

pub(crate) use generate;
