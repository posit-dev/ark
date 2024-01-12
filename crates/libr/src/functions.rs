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

        mod functions_initializer {
            /// Initialize library functions
            ///
            /// If we can't find it in the library, the `Option` wrapper remains `None`.
            /// This indicates that this version of R doesn't have that function.
            pub fn initialize(library: &libloading::Library) {
                $(
                    {
                        paste::paste! {
                            use crate::[<$name _opt>];

                            let symbol = unsafe { library.get(stringify!($name).as_bytes()) };

                            let pointer = match symbol {
                                Ok(symbol) => Some(*symbol),
                                Err(_) => None
                            };

                            unsafe { [<$name _opt>] = pointer };
                        }
                    }
                )+
            }
        }
    }
}

pub(crate) use generate;
