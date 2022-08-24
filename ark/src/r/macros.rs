//
// macros.rs
//
// Copyright (C) 2022 by RStudio, PBC
//
//

// NOTE: We provide an API for Rf_install() as rust's strings are not
// nul-terminated by default, and so we need to do the work to ensure
// the strings we pass to Rf_install() are nul-terminated C strings.
macro_rules! rsymbol {

    ($id:literal) => {{
        let value = concat!($id, "\0");
        Rf_install(value.as_ptr() as *const i8)
    }};

    ($id:expr) => {{
        let cstr = [&*$id, "\0"].concat();
        Rf_install(cstr.as_ptr() as *const i8)
    }};

}
pub(crate) use rsymbol;

macro_rules! rstring {

    ($id:expr) => {{
        let mut protect = RProtect::new();
        let value = &*$id;
        let string_sexp = protect.add(Rf_allocVector(STRSXP, 1));
        let char_sexp = Rf_mkCharLenCE(value.as_ptr() as *const i8, value.len() as i32, cetype_t_CE_UTF8);
        SET_STRING_ELT(string_sexp, 0, char_sexp);
        string_sexp
    }}

}
pub(crate) use rstring;

// Mainly for debugging.
macro_rules! rlog {

    ($x:expr) => {

        let value = $x;
        Rf_PrintValue(value);

        // NOTE: We construct and evaluate the call by hand here
        // just to avoid a potential infinite recursion if this
        // macro were to be used within other R APIs we expose.
        let callee = Rf_protect(Rf_lang3(
            crate::r::macros::rsymbol!("::"),
            crate::r::macros::rstring!("base"),
            crate::r::macros::rstring!("format"),
        ));

        let errc = 0;
        let call = Rf_protect(Rf_lang2(callee, value));
        let result = R_tryEvalSilent(call, R_GlobalEnv, &errc);
        if errc != 0 {
            let robj = extendr_api::Robj::from_sexp(result);
            if let Ok(strings) = extendr_api::Strings::try_from(robj) {
                for string in strings.iter() {
                    crate::lsp::logger::dlog!("{}", string);
                }
            }
        }

        Rf_unprotect(2);

    }

}
pub(crate) use rlog;

