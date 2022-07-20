// 
// macros.rs
// 
// Copyright (C) 2022 by RStudio, PBC
// 
// 

// NOTE: We provide an API for Rf_install() as rust's strings are not
// nul-terminated by default, and so we need to do the work to ensure
// the strings we pass to Rf_install() are nul-terminated C strings.
macro_rules! install {

    ($id:literal) => {{
        let value = concat!($id, "\0");
        rlock! { Rf_install(value.as_ptr() as *const i8) }
    }};

    ($id:expr) => {{
        let cstr = [$id, "\0"].concat();
        rlock! { Rf_install(cstr.as_ptr() as *const i8) }
    }};

}
pub(crate) use install;

macro_rules! rstring {

    ($id:literal) => {{
        rlock! {
            let mut protect = RProtect::new();
            let value = $id;
            let charsexp = protect.add(Rf_mkCharLenCE(value.as_ptr() as *const i8, value.len() as i32, cetype_t_CE_UTF8));
            let strsxp = Rf_allocVector(STRSXP, 1);
            SET_STRING_ELT(strsxp, 0, charsexp);
            strsxp
        }
    }}

}
pub(crate) use rstring;

// Mainly for debugging.
macro_rules! rlog {

    ($x:expr) => {

        rlock! {

            // NOTE: We construct and evaluate the call by hand here
            // just to avoid a potential infinite recursion if this
            // macro were to be used within other R APIs we expose.
            let mut protect = RProtect::new();

            let callee = protect.add(Rf_lang3(
                crate::r::macros::install!("::"),
                crate::r::macros::rstring!("base"),
                crate::r::macros::rstring!("format"),
            ));

            let call = protect.add(Rf_lang2(callee, $x));
            let result = Rf_eval(call, R_GlobalEnv);

            let robj = extendr_api::Robj::from_sexp(result);
            if let Ok(strings) = extendr_api::Strings::try_from(robj) {
                for string in strings.iter() {
                    crate::lsp::logger::dlog!("{}", string);
                }
            }
        }

    }

}
pub(crate) use rlog;

