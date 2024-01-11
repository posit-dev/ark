//
// utils.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::ffi::CStr;
use std::ffi::CString;
use std::os::raw::c_void;
use std::path::Path;

use c2rust_bitfields::BitfieldStruct;
use harp_macros::register;
use itertools::Itertools;
use libR_shim::*;
use once_cell::sync::Lazy;
use regex::Regex;
use stdext::unwrap;

use crate::environment::Environment;
use crate::environment::R_ENVS;
use crate::error::Error;
use crate::error::Result;
use crate::eval::r_parse_eval0;
use crate::exec::geterrmessage;
use crate::exec::RArgument;
use crate::exec::RFunction;
use crate::exec::RFunctionExt;
use crate::object::RObject;
use crate::protect::RProtect;
use crate::r_char;
use crate::r_lang;
use crate::r_symbol;
use crate::string::r_is_string;
use crate::symbol::RSymbol;
use crate::vector::CharacterVector;
use crate::vector::IntegerVector;
use crate::vector::Vector;

pub static mut HARP_ENV: SEXP = std::ptr::null_mut();

// NOTE: Regex::new() is quite slow to compile, so it's much better to keep
// a single singleton pattern and use that repeatedly for matches.
static RE_SYNTACTIC_IDENTIFIER: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[\p{L}\p{Nl}.][\p{L}\p{Nl}\p{Mn}\p{Mc}\p{Nd}\p{Pc}.]*$").unwrap());

extern "C" {
    fn R_removeVarFromFrame(symbol: SEXP, envir: SEXP) -> c_void;
}

#[derive(Copy, Clone, BitfieldStruct)]
#[repr(C)]
pub struct Sxpinfo {
    #[bitfield(name = "rtype", ty = "libc::c_uint", bits = "0..=4")]
    #[bitfield(name = "scalar", ty = "libc::c_uint", bits = "5..=5")]
    #[bitfield(name = "obj", ty = "libc::c_uint", bits = "6..=6")]
    #[bitfield(name = "alt", ty = "libc::c_uint", bits = "7..=7")]
    #[bitfield(name = "gp", ty = "libc::c_uint", bits = "8..=23")]
    #[bitfield(name = "mark", ty = "libc::c_uint", bits = "24..=24")]
    #[bitfield(name = "debug", ty = "libc::c_uint", bits = "25..=25")]
    #[bitfield(name = "trace", ty = "libc::c_uint", bits = "26..=26")]
    #[bitfield(name = "spare", ty = "libc::c_uint", bits = "27..=27")]
    #[bitfield(name = "gcgen", ty = "libc::c_uint", bits = "28..=28")]
    #[bitfield(name = "gccls", ty = "libc::c_uint", bits = "29..=31")]
    #[bitfield(name = "named", ty = "libc::c_uint", bits = "32..=47")]
    #[bitfield(name = "extra", ty = "libc::c_uint", bits = "48..=63")]
    pub rtype_scalar_obj_alt_gp_mark_debug_trace_spare_gcgen_gccls_named_extra: [u8; 8],
}

pub static mut ACTIVE_BINDING_MASK: libc::c_uint = 1 << 15;
pub static mut S4_OBJECT_MASK: libc::c_uint = 1 << 4;
pub static mut HASHASH_MASK: libc::c_uint = 1;

impl Sxpinfo {
    pub fn interpret(x: &SEXP) -> &Self {
        unsafe { (*x as *mut Sxpinfo).as_ref().unwrap() }
    }

    pub fn is_active(&self) -> bool {
        self.gp() & unsafe { ACTIVE_BINDING_MASK } != 0
    }

    pub fn is_immediate(&self) -> bool {
        self.extra() != 0
    }

    pub fn is_s4(&self) -> bool {
        self.gp() & unsafe { S4_OBJECT_MASK } != 0
    }

    pub fn is_altrep(&self) -> bool {
        self.alt() != 0
    }

    pub fn is_object(&self) -> bool {
        self.obj() != 0
    }
}

// Necessary for the `harp` reference in the `register` macro to resolve correctly
mod harp {
    pub use crate::*;
}

#[register]
pub extern "C" fn harp_log_warning(msg: SEXP) -> crate::error::Result<SEXP> {
    let msg = String::try_from(RObject::view(msg))?;
    log::warn!("{msg}");

    unsafe { Ok(R_NilValue) }
}

#[register]
pub extern "C" fn harp_log_error(msg: SEXP) -> crate::error::Result<SEXP> {
    let msg = String::try_from(RObject::view(msg))?;
    log::error!("{msg}");

    unsafe { Ok(R_NilValue) }
}

pub fn r_assert_type(object: SEXP, expected: &[u32]) -> Result<u32> {
    let actual = r_typeof(object);

    if !expected.contains(&actual) {
        return Err(Error::UnexpectedType(actual, expected.to_vec()));
    }

    Ok(actual)
}

pub unsafe fn r_assert_capacity(object: SEXP, required: usize) -> Result<usize> {
    let actual = Rf_length(object) as usize;
    if actual < required {
        return Err(Error::UnexpectedLength(actual, required));
    }

    Ok(actual)
}

pub fn r_assert_length(object: SEXP, expected: usize) -> Result<usize> {
    let actual = unsafe { Rf_xlength(object) as usize };
    if actual != expected {
        return Err(Error::UnexpectedLength(actual, expected));
    }

    Ok(actual)
}

pub fn r_is_data_frame(object: SEXP) -> bool {
    r_typeof(object) == VECSXP && r_inherits(object, "data.frame")
}

pub fn r_is_null(object: SEXP) -> bool {
    unsafe { object == R_NilValue }
}

pub fn r_is_altrep(object: SEXP) -> bool {
    Sxpinfo::interpret(&object).is_altrep()
}

pub fn r_is_object(object: SEXP) -> bool {
    Sxpinfo::interpret(&object).is_object()
}

pub fn r_is_s4(object: SEXP) -> bool {
    Sxpinfo::interpret(&object).is_s4()
}

pub fn r_is_unbound(object: SEXP) -> bool {
    object == unsafe { R_UnboundValue }
}

pub fn r_is_simple_vector(value: SEXP) -> bool {
    unsafe {
        let class = Rf_getAttrib(value, R_ClassSymbol);

        match r_typeof(value) {
            LGLSXP | REALSXP | CPLXSXP | STRSXP | RAWSXP => r_is_null(class),
            INTSXP => r_is_null(class) || r_inherits(value, "factor"),

            _ => false,
        }
    }
}

pub fn r_is_matrix(value: SEXP) -> bool {
    unsafe { Rf_isMatrix(value) == Rboolean_TRUE }
}

pub fn r_classes(value: SEXP) -> Option<CharacterVector> {
    unsafe {
        let classes = RObject::from(Rf_getAttrib(value, R_ClassSymbol));

        if *classes == R_NilValue {
            None
        } else {
            Some(CharacterVector::new_unchecked(classes))
        }
    }
}

/// Translates a UTF-8 string from an R character vector to a Rust string.
///
/// - `x` is the R vector to translate from.
/// - `i` is the index in the vector of the string to translate.
pub fn r_chr_get_owned_utf8(x: *mut SEXPREC, i: isize) -> Result<String> {
    unsafe { r_str_to_owned_utf8(STRING_ELT(x, i)) }
}

/// Translates an R string to a UTF-8 Rust string.
///
/// - `x` is a CHARSXP.
///
/// Missing values return an `Error::MissingValueError`.
pub fn r_str_to_owned_utf8(x: SEXP) -> Result<String> {
    unsafe {
        if x == R_NaString {
            return Err(Error::MissingValueError);
        }

        // Translate it to a UTF-8 C string (note that this allocates with
        // `R_alloc()` so we need to save and reset the protection stack)
        let vmax = vmaxget();
        let translated = Rf_translateCharUTF8(x);
        vmaxset(vmax);

        // Convert to a Rust string and return
        let cstr = CStr::from_ptr(translated).to_str()?;
        Ok(cstr.to_string())
    }
}

/// Translates an R string to a UTF-8 Rust string without type checking.
///
/// - `x` is a CHARSXP that is assumed to not be missing.
///
/// Uses `from_utf8_unchecked()`.
pub fn r_str_to_owned_utf8_unchecked(x: SEXP) -> String {
    unsafe {
        let vmax = vmaxget();
        let translated = Rf_translateCharUTF8(x);
        vmaxset(vmax);

        let bytes = CStr::from_ptr(translated).to_bytes();
        std::str::from_utf8_unchecked(bytes).to_owned()
    }
}

pub fn pairlist_size(mut pairlist: SEXP) -> Result<isize> {
    let mut n = 0;
    unsafe {
        while pairlist != R_NilValue {
            r_assert_type(pairlist, &[LISTSXP])?;

            pairlist = CDR(pairlist);
            n = n + 1;
        }
    }
    Ok(n)
}

pub fn r_vec_is_single_dimension_with_single_value(value: SEXP) -> bool {
    unsafe { Rf_getAttrib(value, R_DimSymbol) == R_NilValue && Rf_xlength(value) == 1 }
}

pub fn r_vec_type(value: SEXP) -> String {
    match r_typeof(value) {
        INTSXP => unsafe {
            if r_inherits(value, "factor") {
                let levels = Rf_getAttrib(value, R_LevelsSymbol);
                format!("fct({})", XLENGTH(levels))
            } else {
                String::from("int")
            }
        },
        REALSXP => String::from("dbl"),
        LGLSXP => String::from("lgl"),
        STRSXP => String::from("str"),
        RAWSXP => String::from("raw"),
        CPLXSXP => String::from("cplx"),

        // TODO: this should not happen
        _ => String::from("???"),
    }
}

pub fn r_vec_shape(value: SEXP) -> String {
    unsafe {
        let dim = RObject::new(Rf_getAttrib(value, R_DimSymbol));

        if r_is_null(*dim) {
            format!("{}", Rf_xlength(value))
        } else {
            let dim = IntegerVector::new_unchecked(*dim);
            dim.iter().map(|d| d.unwrap()).join(", ")
        }
    }
}

pub fn r_altrep_class(object: SEXP) -> String {
    let serialized_klass = unsafe { ATTRIB(ALTREP_CLASS(object)) };

    let klass = RSymbol::new_unchecked(unsafe { CAR(serialized_klass) });
    let pkg = RSymbol::new_unchecked(unsafe { CADR(serialized_klass) });

    format!("{}::{}", pkg, klass)
}

pub fn r_typeof(object: SEXP) -> u32 {
    // SAFETY: The type of an R object is typically considered constant,
    // and TYPEOF merely queries the R type directly from the SEXPREC struct.
    let object = object.into();
    unsafe { TYPEOF(object) as u32 }
}

pub unsafe fn r_type2char<T: Into<u32>>(kind: T) -> String {
    let kind = Rf_type2char(kind.into());
    let cstr = CStr::from_ptr(kind);
    return cstr.to_str().unwrap().to_string();
}

pub unsafe fn r_get_option<T: TryFrom<RObject, Error = Error>>(name: &str) -> Result<T> {
    let result = Rf_GetOption1(r_symbol!(name));
    return RObject::new(result).try_into();
}

pub fn r_inherits(object: SEXP, class: &str) -> bool {
    let class = CString::new(class).unwrap();
    unsafe { Rf_inherits(object, class.as_ptr()) != 0 }
}

pub fn r_is_function(object: SEXP) -> bool {
    matches!(r_typeof(object), CLOSXP | BUILTINSXP | SPECIALSXP)
}

pub unsafe fn r_formals(object: SEXP) -> Result<Vec<RArgument>> {
    // convert primitive functions into equivalent closures
    let mut object = RObject::new(object);
    if r_typeof(*object) == BUILTINSXP || r_typeof(*object) == SPECIALSXP {
        object = RFunction::new("base", "args").add(*object).call()?;
        if r_typeof(*object) != CLOSXP {
            return Ok(Vec::new());
        }
    }

    // validate we have a closure now
    r_assert_type(*object, &[CLOSXP])?;

    // get the formals
    let mut formals = FORMALS(*object);

    // iterate through the entries
    let mut arguments = Vec::new();

    while formals != R_NilValue {
        let name = RObject::from(TAG(formals)).to::<String>()?;
        let value = CAR(formals);
        arguments.push(RArgument::new(name.as_str(), RObject::new(value)));
        formals = CDR(formals);
    }

    Ok(arguments)
}

pub unsafe fn r_envir_name(envir: SEXP) -> Result<String> {
    r_assert_type(envir, &[ENVSXP])?;

    if r_env_is_pkg_env(envir) {
        let name = RObject::from(r_pkg_env_name(envir));
        return name.to::<String>();
    }

    if r_env_is_ns_env(envir) {
        let name = RObject::from(r_ns_env_name(envir));
        return name.to::<String>();
    }

    let name = Rf_getAttrib(envir, r_symbol!("name"));
    if r_typeof(name) == STRSXP {
        let name = RObject::view(name).to::<String>()?;
        return Ok(name);
    }

    Ok(format!("{:p}", envir))
}

pub unsafe fn r_envir_get(symbol: &str, envir: SEXP) -> Option<SEXP> {
    let value = Rf_findVar(r_symbol!(symbol), envir);
    if value == R_UnboundValue {
        return None;
    }

    Some(value)
}

pub unsafe fn r_envir_set(symbol: &str, value: SEXP, envir: SEXP) {
    Rf_defineVar(r_symbol!(symbol), value, envir);
}

pub unsafe fn r_envir_remove(symbol: &str, envir: SEXP) {
    R_removeVarFromFrame(r_symbol!(symbol), envir);
}

pub unsafe fn r_stringify(object: SEXP, delimiter: &str) -> Result<String> {
    // handle SYMSXPs upfront
    if r_typeof(object) == SYMSXP {
        return RObject::view(object).to::<String>();
    }

    // call format on the object
    let object = RFunction::new("base", "format").add(object).call()?;

    // paste into a single string
    let object = RFunction::new("base", "paste")
        .add(object)
        .param("collapse", delimiter)
        .call()?
        .to::<String>()?;

    Ok(object)
}

pub unsafe fn r_inspect(object: SEXP) {
    let mut protect = RProtect::new();
    let inspect = protect.add(Rf_lang2(r_symbol!("inspect"), object));
    let internal = protect.add(Rf_lang2(r_symbol!(".Internal"), inspect));
    Rf_eval(internal, R_BaseEnv);
}

pub fn r_symbol_valid(name: &str) -> bool {
    RE_SYNTACTIC_IDENTIFIER.is_match(name)
}

pub fn r_symbol_quote_invalid(name: &str) -> String {
    if RE_SYNTACTIC_IDENTIFIER.is_match(name) {
        name.to_string()
    } else {
        r_symbol_quote(name)
    }
}

pub fn r_symbol_quote(name: &str) -> String {
    format!("`{}`", name.replace("`", "\\`"))
}

pub unsafe fn r_promise_is_forced(x: SEXP) -> bool {
    PRVALUE(x) != R_UnboundValue
}

pub unsafe fn r_promise_force(x: SEXP) -> Result<SEXP> {
    // Expect that the promise protects its own result
    r_try_eval_silent(x, R_EmptyEnv)
}

pub unsafe fn r_promise_force_with_rollback(x: SEXP) -> Result<SEXP> {
    // Like `r_promise_force()`, but if evaluation results in an error
    // then the original promise is untouched, i.e. `PRSEEN` isn't modified,
    // avoiding `"restarting interrupted promise evaluation"` warnings.
    let out = r_try_eval_silent(PRCODE(x), PRENV(x))?;
    SET_PRVALUE(x, out);
    Ok(out)
}

pub unsafe fn r_promise_is_lazy_load_binding(x: SEXP) -> bool {
    // `rlang:::promise_expr("across", asNamespace("dplyr"))`
    // returns:
    // `lazyLoadDBfetch(c(105202L, 4670L), datafile, compressed, envhook)`
    // We can take advantage of this to identify promises in namespaces
    // that correspond to symbols we should evaluate when generating completions.

    let expr = PRCODE(x);

    if r_typeof(expr) != LANGSXP {
        return false;
    }

    if Rf_xlength(expr) == 0 {
        return false;
    }

    let expr = CAR(expr);

    if r_typeof(expr) != SYMSXP {
        return false;
    }

    expr == r_symbol!("lazyLoadDBfetch")
}

pub unsafe fn r_env_has(env: SEXP, sym: SEXP) -> bool {
    if libr::R_existsVarInFrame::is_loaded() {
        libr::R_existsVarInFrame(env, sym) == libr::Rboolean_TRUE
    } else {
        // Not particularly fast, but seems to be good enough for checking symbol
        // existance during completion generation
        let mut protect = RProtect::new();
        let name = protect.add(PRINTNAME(sym));
        let name = protect.add(Rf_ScalarString(name));
        let call = protect.add(r_lang!(
            r_symbol!("exists"),
            x = name,
            envir = env,
            inherits = false
        ));
        let out = Rf_eval(call, R_BaseEnv);
        // `exists()` is guaranteed to return a logical vector on success
        LOGICAL_ELT(out, 0) != 0
    }
}

pub unsafe fn r_env_binding_is_active(env: SEXP, sym: SEXP) -> bool {
    R_BindingIsActive(sym, env) == Rboolean_TRUE
}

pub unsafe fn r_env_is_pkg_env(env: SEXP) -> bool {
    R_IsPackageEnv(env) == Rboolean_TRUE || env == R_BaseEnv
}

pub unsafe fn r_pkg_env_name(env: SEXP) -> SEXP {
    if env == R_BaseEnv {
        // `R_BaseEnv` is not handled by `R_PackageEnvName()`, but most of the time we want to
        // treat it like a package namespace
        return r_char!("base");
    }

    let name = R_PackageEnvName(env);

    if name == R_NilValue {
        // Should be very unlikely, but `NULL` can be returned
        return r_char!("");
    }

    STRING_ELT(name, 0)
}

pub unsafe fn r_env_is_ns_env(env: SEXP) -> bool {
    // Does handle `R_BaseNamespace`
    // https://github.com/wch/r-source/blob/1cb35ff692d3eb3ab546e0db4761102b5ea4ac89/src/main/envir.c#L3689
    R_IsNamespaceEnv(env) == Rboolean_TRUE
}

pub unsafe fn r_ns_env_name(env: SEXP) -> SEXP {
    // Does handle `R_BaseNamespace`
    // https://github.com/wch/r-source/blob/1cb35ff692d3eb3ab546e0db4761102b5ea4ac89/src/main/envir.c#L3720
    let mut protect = RProtect::new();

    let spec = protect.add(R_NamespaceEnvSpec(env));

    if spec == R_NilValue {
        // Should be very unlikely, but `NULL` can be returned
        return r_char!("");
    }

    STRING_ELT(spec, 0)
}

pub unsafe fn r_try_eval_silent(x: SEXP, env: SEXP) -> Result<SEXP> {
    let mut errc = 0;

    let x = R_tryEvalSilent(x, env, &mut errc);

    // NOTE: This error message is potentially incorrect because `errc`
    // might be true in other cases of longjumps than just errors.
    if errc != 0 {
        return Err(Error::TryEvalError {
            message: geterrmessage(),
        });
    }

    Ok(x)
}

static mut OPTIONS_FN: Option<SEXP> = None;

// Note this might throw if wrong data types are passed in. The C-level
// implementation of `options()` type-checks some base options.
pub fn r_poke_option(sym: SEXP, value: SEXP) -> SEXP {
    unsafe {
        let mut protect = RProtect::new();

        let call = r_lang!(OPTIONS_FN.unwrap_unchecked(), !!sym = value);
        protect.add(call);

        // `options()` is guaranteed by R to return a list
        VECTOR_ELT(Rf_eval(call, R_BaseEnv), 0)
    }
}

pub fn r_poke_option_show_error_messages(value: bool) -> bool {
    unsafe {
        let value = Rf_ScalarLogical(value as i32);
        let old = r_poke_option(r_symbol!("show.error.messages"), value);

        // This option is type-checked by R so we can assume a valid
        // logical value
        *LOGICAL(old) != 0
    }
}

pub fn r_normalize_path(x: RObject) -> anyhow::Result<String> {
    if !r_is_string(x.sexp) {
        anyhow::bail!("Expected string for srcfile's filename");
    }
    unsafe {
        let path = RFunction::new("base", "normalizePath")
            .param("path", x)
            .param("winslash", "/")
            .param("mustWork", false)
            .call()?
            .to::<String>()?;
        Ok(path)
    }
}

pub fn init_utils() {
    init_modules();

    unsafe {
        let options_fn = Rf_eval(r_symbol!("options"), R_BaseEnv);
        OPTIONS_FN = Some(options_fn);
    }
}

// Largely copied from `module.rs` in the Ark crate
fn init_modules() {
    unsafe {
        let ns = r_parse_eval0("new.env()", R_ENVS.base).unwrap();
        R_PreserveObject(ns.sexp);
        HARP_ENV = ns.sexp;
    }

    // Get the path to the 'modules' directory, adjacent to the executable file.
    // This is where we place the R source files in packaged releases.
    let mut root = match std::env::current_exe() {
        Ok(exe_path) => exe_path.parent().unwrap().join("modules"),
        Err(err) => {
            log::error!("Failed to get current exe path; can't find harp modules: {err:?}");
            return;
        },
    };

    // If that path doesn't exist, we're probably running from source, so
    // look in the source tree (found via the 'CARGO_MANIFEST_DIR' environment
    // variable).
    if !root.exists() {
        let source = env!("CARGO_MANIFEST_DIR");
        root = Path::new(&source).join("src").join("modules").to_path_buf();
    }

    log::info!("Loading modules from directory: {}", root.display());
    let entries = unwrap!(std::fs::read_dir(root), Err(err) => {
        log::error!("Can't read module directory: {err:?}");
        return;
    });

    for entry in entries {
        let entry = unwrap!(
            entry,
            Err(err) => {
                log::error!("Can't load directory entry due to: {}", err);
                continue;
            }
        );

        let path = entry.path();

        if path.extension().is_some_and(|ext| ext == "R") {
            source_file(&path).unwrap();
        }
    }
}

fn source_file(file: &Path) -> anyhow::Result<()> {
    let file = file.to_str().unwrap();
    unsafe {
        RFunction::new("base", "sys.source")
            .param("file", file)
            .param("envir", HARP_ENV)
            .call()?;
    }

    Ok(())
}

pub fn save_rds(x: SEXP, path: &str) {
    let path = RObject::from(path);

    let env = Environment::new(r_parse_eval0("new.env()", R_ENVS.base).unwrap());
    env.bind("x", x);
    env.bind("path", path);

    let res = r_parse_eval0("base::saveRDS(x, path)", env);

    // This is meant for internal use so report errors loudly
    res.unwrap();
}

/// Meant for debugging inside lldb. Since we can't call C functions reliably
/// (let me know if you find a way), Inserting `push_rds()` in your code lets
/// you save objects that you can then inspect from R.
///
/// The objects are pushed to a data frame with newer entries preserved in
/// earlier rows, with a datetime and optional context attached.
///
/// If `path` is empty, saves RDS in the path stored in the
/// `RUST_PUSH_RDS_PATH` environment variable.
pub fn push_rds(x: SEXP, path: &str, context: &str) {
    let path = if path.len() == 0 {
        RObject::null()
    } else {
        RObject::from(path)
    };
    let context = RObject::from(context);

    let env = Environment::new(r_parse_eval0("new.env()", R_ENVS.global).unwrap());

    env.bind("x", x);
    env.bind("path", path);
    env.bind("context", context);

    let res = r_parse_eval0(".ps.internal(push_rds(x, path, context))", env);

    // This is meant for internal use so report errors loudly
    res.unwrap();
}
