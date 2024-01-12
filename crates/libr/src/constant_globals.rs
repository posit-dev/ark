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

        mod constant_globals_initializer {
            /// Initialize library constant globals
            pub fn initialize(library: &libloading::Library) {
                $(
                    {
                        use crate::$name;
                        paste::paste! {
                            // All of our types are also idents, we paste to generate the
                            // ident for import purposes
                            use crate::[<$ty>];
                        }

                        let symbol = unsafe { library.get(stringify!($name).as_bytes()) };

                        // TODO: Handle missing case

                        // Pull into Rust as a constant pointer
                        let pointer: *const $ty = match symbol {
                            Ok(symbol) => *symbol,
                            Err(_) => panic!("Missing constant global")
                        };

                        // Assume global has been initialized on the R side, and is
                        // otherwise constant, so deref to copy it over and assign it once.
                        unsafe { $name = *pointer };
                    }
                )+
            }
        }
    );
}

pub(crate) use generate;
