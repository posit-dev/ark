//
// lib.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

macro_rules! link {
    // `@LOAD_FUNCTION` internal entry point
    // Generates the one time loading function for a particular library function entry
    (
        @LOAD_FUNCTION:
        $(#[cfg($cfg:meta)])*
        fn $name:ident($($pname:ident: $pty:ty), *) $(-> $ret:ty)*
    ) => (
        $(#[cfg($cfg)])*
        pub fn $name(library: &mut super::SharedLibrary) {
            let symbol = unsafe { library.library.get(stringify!($name).as_bytes()) }.ok();
            library.functions.$name = match symbol {
                Some(s) => *s,
                None => None,
            };
        }
    );

    // `@LOAD_GLOBAL` internal entry point
    // Generates the one time loading function for a particular library global entry
    (
        @LOAD_GLOBAL:
        $(#[cfg($gcfg:meta)])*
        static mut $gname:ident: $gty:ty
    ) => (
        $(#[cfg($gcfg)])*
        pub fn $gname(library: &mut super::SharedLibrary) {
            let symbol = unsafe { library.library.get(stringify!($gname).as_bytes()) }.ok();
            library.globals.$gname = match symbol {
                Some(s) => *s,
                None => None,
            };
        }
    );

    // Main entry point
    (
        $(
            $(#[doc=$doc:expr])*
            $(#[cfg($cfg:meta)])*
            pub fn $name:ident($($pname:ident: $pty:ty), *) $(-> $ret:ty)*;
        )+
        $(
            $(#[doc=$gdoc:expr])*
            $(#[cfg($gcfg:meta)])*
            pub static mut $gname:ident: $gty:ty;
        )+
    ) => (
        use std::cell::{RefCell};
        use std::sync::{Arc};
        use std::path::{Path, PathBuf};

        /// The set of functions loaded dynamically.
        #[derive(Debug, Default)]
        pub struct Functions {
            $(
                $(#[doc=$doc])*
                $(#[cfg($cfg)])*
                pub $name: Option<unsafe extern fn($($pname: $pty), *) $(-> $ret)*>,
            )+
        }

        /// The set of globals loaded dynamically.
        #[derive(Debug, Default)]
        pub struct Globals {
            $(
                $(#[doc=$gdoc])*
                $(#[cfg($gcfg)])*
                pub $gname: Option<*mut $gty>,
            )+
        }

        /// A dynamically loaded instance of the `R` library.
        #[derive(Debug)]
        pub struct SharedLibrary {
            library: libloading::Library,
            path: PathBuf,
            functions: Functions,
            globals: Globals,
        }

        impl SharedLibrary {
            fn new(library: libloading::Library, path: PathBuf) -> Self {
                Self { library, path, functions: Functions::default(), globals: Globals::default() }
            }

            /// Returns the path to this `R` shared library.
            pub fn path(&self) -> &Path {
                &self.path
            }
        }

        thread_local!(static LIBRARY: RefCell<Option<Arc<SharedLibrary>>> = RefCell::new(None));

        /// Returns whether an `R` shared library is loaded on this thread.
        pub fn is_loaded() -> bool {
            LIBRARY.with(|l| l.borrow().is_some())
        }

        fn with_library<T, F>(f: F) -> Option<T> where F: FnOnce(&SharedLibrary) -> T {
            LIBRARY.with(|l| {
                match l.borrow().as_ref() {
                    Some(library) => Some(f(&library)),
                    _ => None,
                }
            })
        }

        $(
            #[cfg_attr(feature="cargo-clippy", allow(clippy::missing_safety_doc))]
            #[cfg_attr(feature="cargo-clippy", allow(clippy::too_many_arguments))]
            $(#[doc=$doc])*
            $(#[cfg($cfg)])*
            pub unsafe fn $name($($pname: $pty), *) $(-> $ret)* {
                let f = with_library(|library| {
                    if let Some(function) = library.functions.$name {
                        function
                    } else {
                        panic!(
                            r#"
An `R` function was called that is not supported by the loaded `R` instance.

    called function = `{0}`

Check the `R` version you are running against this function.
Check the OS platform you are using against this function.
"#,
                            stringify!($name),
                        );
                    }
                }).expect("an `R` shared library is not loaded on this thread");
                f($($pname), *)
            }

            $(#[doc=$doc])*
            $(#[cfg($cfg)])*
            pub mod $name {
                pub fn is_loaded() -> bool {
                    super::with_library(|l| l.functions.$name.is_some()).unwrap_or(false)
                }
            }
        )+

        $(
            #[cfg_attr(feature="cargo-clippy", allow(clippy::missing_safety_doc))]
            #[cfg_attr(feature="cargo-clippy", allow(clippy::too_many_arguments))]
            $(#[doc=$gdoc])*
            $(#[cfg($gcfg)])*
            pub unsafe fn $gname() -> *mut $gty {
                with_library(|library| {
                    if let Some(global) = library.globals.$gname {
                        global
                    } else {
                        panic!(
                            r#"
An `R` global was called that is not supported by the loaded `R` instance.

    called global = `{0}`

Check the `R` version you are running against this global.
Check the OS platform you are using against this global.
"#,
                            stringify!($gname),
                        );
                    }
                }).expect("an `R` shared library is not loaded on this thread")
            }

            $(#[doc=$gdoc])*
            $(#[cfg($gcfg)])*
            pub mod $gname {
                pub fn is_loaded() -> bool {
                    super::with_library(|l| l.globals.$gname.is_some()).unwrap_or(false)
                }
            }
        )+

        mod load_function {
            $(link!(@LOAD_FUNCTION: $(#[cfg($cfg)])* fn $name($($pname: $pty), *) $(-> $ret)*);)+
        }
        mod load_global {
            $(link!(@LOAD_GLOBAL: $(#[cfg($gcfg)])* static mut $gname: $gty);)+
        }

        /// Loads an `R` shared library and returns the library instance.
        ///
        /// # Failures
        ///
        /// * an `R` shared library could not be found
        /// * the `R` shared library could not be opened
        fn load_manually(path: PathBuf) -> Result<SharedLibrary, String> {
            unsafe {
                let library = libloading::Library::new(&path).map_err(|err| {
                    format!(
                        "the `R` shared library at {} could not be opened: {}",
                        path.display(),
                        err,
                    )
                });

                let mut library = SharedLibrary::new(library?, path);

                // Perform initial loading of all functions and globals
                $(load_function::$name(&mut library);)+
                $(load_global::$gname(&mut library);)+

                Ok(library)
            }
        }

        /// Loads an `R` shared library for use in the current thread.
        ///
        /// This functions attempts to load all the functions in the shared library. Whether a
        /// function has been loaded can be tested by calling the `is_loaded` function on the
        /// module with the same name as the function (e.g., `Rf_error::is_loaded()` for
        /// the `Rf_error` function).
        ///
        /// # Failures
        ///
        /// * an `R` shared library could not be found
        /// * the `R` shared library could not be opened
        pub fn load(path: PathBuf) -> Result<(), String> {
            let library = Arc::new(load_manually(path)?);
            LIBRARY.with(|l| *l.borrow_mut() = Some(library));
            Ok(())
        }

        /// Unloads the `R` shared library in use in the current thread.
        ///
        /// # Failures
        ///
        /// * an `R` shared library is not in use in the current thread
        pub fn unload() -> Result<(), String> {
            let library = set_library(None);
            if library.is_some() {
                Ok(())
            } else {
                Err("an `R` shared library is not in use in the current thread".into())
            }
        }

        /// Returns the library instance stored in TLS.
        ///
        /// This functions allows for sharing library instances between threads.
        pub fn get_library() -> Option<Arc<SharedLibrary>> {
            LIBRARY.with(|l| l.borrow_mut().clone())
        }

        /// Sets the library instance stored in TLS and returns the previous library.
        ///
        /// This functions allows for sharing library instances between threads.
        pub fn set_library(library: Option<Arc<SharedLibrary>>) -> Option<Arc<SharedLibrary>> {
            LIBRARY.with(|l| std::mem::replace(&mut *l.borrow_mut(), library))
        }
    )
}
