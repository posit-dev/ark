//
// r.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

#![allow(non_upper_case_globals)]
#![allow(non_snake_case)]

use crate::constant_globals;
use crate::functions;
use crate::functions_variadic;
use crate::graphics::pGEDevDesc;
use crate::mutable_globals;
use crate::types::*;

// ---------------------------------------------------------------------------------------
// Functions and globals

functions::generate! {
    pub fn Rf_initialize_R(ac: std::ffi::c_int, av: *mut *mut std::ffi::c_char) -> std::ffi::c_int;

    pub fn run_Rmainloop();

    pub fn setup_Rmainloop();

    pub fn R_HomeDir() -> *mut std::ffi::c_char;

    pub fn R_ProcessEvents();

    pub fn R_removeVarFromFrame(symbol: SEXP, envir: SEXP);

    pub fn R_getEmbeddingDllInfo() -> *mut DllInfo;

    pub fn R_registerRoutines(
        info: *mut DllInfo,
        croutines: *const R_CMethodDef,
        callRoutines: *const R_CallMethodDef,
        fortranRoutines: *const R_FortranMethodDef,
        externalRoutines: *const R_ExternalMethodDef
    ) -> std::ffi::c_int;

    pub fn vmaxget() -> *mut std::ffi::c_void;

    pub fn vmaxset(arg1: *const std::ffi::c_void);

    pub fn R_BindingIsActive(sym: SEXP, env: SEXP) -> Rboolean;

    pub fn R_CheckStack();

    pub fn R_CheckStack2(arg1: usize);

    pub fn R_CheckUserInterrupt();

    pub fn R_ExternalPtrAddr(s: SEXP) -> *mut std::ffi::c_void;

    pub fn R_MakeExternalPtr(p: *mut std::ffi::c_void, tag: SEXP, prot: SEXP) -> SEXP;

    pub fn R_IsNA(arg1: f64) -> std::ffi::c_int;

    pub fn R_IsNaN(arg1: f64) -> std::ffi::c_int;

    pub fn R_finite(arg1: f64) -> std::ffi::c_int;

    pub fn R_IsNamespaceEnv(rho: SEXP) -> Rboolean;

    pub fn R_IsPackageEnv(rho: SEXP) -> Rboolean;

    pub fn R_NamespaceEnvSpec(rho: SEXP) -> SEXP;

    pub fn R_PackageEnvName(rho: SEXP) -> SEXP;

    pub fn R_ParseVector(
        arg1: SEXP,
        arg2: std::ffi::c_int,
        arg3: *mut ParseStatus,
        arg4: SEXP
    ) -> SEXP;

    pub fn R_PreserveObject(arg1: SEXP);

    pub fn R_RunPendingFinalizers();

    pub fn R_ToplevelExec(
        fun: Option<unsafe extern "C" fn(arg1: *mut std::ffi::c_void)>,
        data: *mut std::ffi::c_void
    ) -> Rboolean;

    pub fn R_withCallingErrorHandler(
        body: Option<unsafe extern "C" fn(args: *mut std::ffi::c_void) -> SEXP>,
        bdata: *mut std::ffi::c_void,
        handler: Option<unsafe extern "C" fn(err: SEXP, args: *mut std::ffi::c_void) -> SEXP>,
        hdata: *mut std::ffi::c_void
    ) -> SEXP;

    pub fn R_altrep_data1(x: SEXP) -> SEXP;

    pub fn R_altrep_data2(x: SEXP) -> SEXP;

    pub fn R_curErrorBuf() -> *const std::ffi::c_char;

    pub fn R_do_slot(obj: SEXP, name: SEXP) -> SEXP;

    pub fn R_lsInternal(arg1: SEXP, arg2: Rboolean) -> SEXP;

    pub fn R_lsInternal3(x: SEXP, all: Rboolean, sorted: Rboolean) -> SEXP;

    pub fn Rf_GetOption1(arg1: SEXP) -> SEXP;

    pub fn Rf_ScalarInteger(arg1: std::ffi::c_int) -> SEXP;

    pub fn Rf_ScalarLogical(arg1: std::ffi::c_int) -> SEXP;

    pub fn Rf_ScalarReal(arg1: f64) -> SEXP;

    pub fn Rf_ScalarString(arg1: SEXP) -> SEXP;

    pub fn Rf_allocVector(arg1: SEXPTYPE, arg2: R_xlen_t) -> SEXP;

    pub fn Rf_asInteger(x: SEXP) -> std::ffi::c_int;

    pub fn Rf_coerceVector(arg1: SEXP, arg2: SEXPTYPE) -> SEXP;

    pub fn Rf_cons(arg1: SEXP, arg2: SEXP) -> SEXP;

    pub fn Rf_defineVar(arg1: SEXP, arg2: SEXP, arg3: SEXP);

    pub fn Rf_eval(arg1: SEXP, arg2: SEXP) -> SEXP;

    pub fn Rf_findVar(arg1: SEXP, arg2: SEXP) -> SEXP;

    pub fn Rf_findVarInFrame(arg1: SEXP, arg2: SEXP) -> SEXP;

    pub fn Rf_getAttrib(arg1: SEXP, arg2: SEXP) -> SEXP;

    pub fn Rf_duplicate(arg: SEXP) -> SEXP;

    pub fn Rf_shallow_duplicate(arg: SEXP) -> SEXP;

    pub fn Rf_inherits(arg1: SEXP, arg2: *const std::ffi::c_char) -> Rboolean;

    pub fn Rf_install(arg1: *const std::ffi::c_char) -> SEXP;

    pub fn Rf_isFunction(arg1: SEXP) -> Rboolean;

    pub fn Rf_isInteger(arg1: SEXP) -> Rboolean;

    pub fn Rf_isMatrix(arg1: SEXP) -> Rboolean;

    pub fn Rf_isNumeric(arg1: SEXP) -> Rboolean;

    pub fn Rf_isString(s: SEXP) -> Rboolean;

    pub fn Rf_lang1(arg1: SEXP) -> SEXP;

    pub fn Rf_lang2(arg1: SEXP, arg2: SEXP) -> SEXP;

    pub fn Rf_lang3(arg1: SEXP, arg2: SEXP, arg3: SEXP) -> SEXP;

    pub fn Rf_lang4(arg1: SEXP, arg2: SEXP, arg3: SEXP, arg4: SEXP) -> SEXP;

    pub fn Rf_lcons(arg1: SEXP, arg2: SEXP) -> SEXP;

    pub fn Rf_mkCharLenCE(
        arg1: *const std::ffi::c_char,
        arg2: std::ffi::c_int,
        arg3: cetype_t
    ) -> SEXP;

    pub fn Rf_mkString(arg1: *const std::ffi::c_char) -> SEXP;

    pub fn Rf_onintr();

    pub fn Rf_protect(arg1: SEXP) -> SEXP;

    pub fn Rf_setAttrib(arg1: SEXP, arg2: SEXP, arg3: SEXP) -> SEXP;

    pub fn Rf_setVar(arg1: SEXP, arg2: SEXP, arg3: SEXP);

    pub fn Rf_translateCharUTF8(arg1: SEXP) -> *const std::ffi::c_char;

    pub fn Rf_type2char(arg1: SEXPTYPE) -> *const std::ffi::c_char;

    pub fn Rf_unprotect(arg1: std::ffi::c_int);

    pub fn Rf_xlength(arg1: SEXP) -> R_xlen_t;

    pub fn ALTREP(x: SEXP) -> std::ffi::c_int;

    pub fn ALTREP_CLASS(x: SEXP) -> SEXP;

    pub fn ATTRIB(x: SEXP) -> SEXP;

    pub fn CADDDR(e: SEXP) -> SEXP;

    pub fn CADDR(e: SEXP) -> SEXP;

    pub fn CADR(e: SEXP) -> SEXP;

    pub fn CAR(e: SEXP) -> SEXP;

    pub fn CDDDR(e: SEXP) -> SEXP;

    pub fn CDDR(e: SEXP) -> SEXP;

    pub fn CDR(e: SEXP) -> SEXP;

    pub fn COMPLEX_ELT(x: SEXP, i: R_xlen_t) -> Rcomplex;

    pub fn DATAPTR(x: SEXP) -> *mut std::ffi::c_void;

    pub fn ENCLOS(x: SEXP) -> SEXP;

    pub fn SET_ENCLOS(x: SEXP, v: SEXP) -> SEXP;

    pub fn FORMALS(x: SEXP) -> SEXP;

    pub fn FRAME(x: SEXP) -> SEXP;

    pub fn HASHTAB(x: SEXP) -> SEXP;

    pub fn INTEGER(x: SEXP) -> *mut std::ffi::c_int;

    pub fn INTEGER_ELT(x: SEXP, i: R_xlen_t) -> std::ffi::c_int;

    pub fn LOGICAL(x: SEXP) -> *mut std::ffi::c_int;

    pub fn LOGICAL_ELT(x: SEXP, i: R_xlen_t) -> std::ffi::c_int;

    pub fn PRCODE(x: SEXP) -> SEXP;

    pub fn PRENV(x: SEXP) -> SEXP;

    pub fn PRINTNAME(x: SEXP) -> SEXP;

    pub fn PRVALUE(x: SEXP) -> SEXP;

    pub fn IS_S4_OBJECT(x: SEXP) -> std::ffi::c_int;

    pub fn RAW(x: SEXP) -> *mut Rbyte;

    pub fn RAW_ELT(x: SEXP, i: R_xlen_t) -> Rbyte;

    pub fn RDEBUG(x: SEXP) -> std::ffi::c_int;

    pub fn REAL(x: SEXP) -> *mut f64;

    pub fn REAL_ELT(x: SEXP, i: R_xlen_t) -> f64;

    pub fn R_CHAR(x: SEXP) -> *const std::ffi::c_char;

    pub fn SETCAR(x: SEXP, y: SEXP) -> SEXP;

    pub fn SETCDR(x: SEXP, y: SEXP) -> SEXP;

    pub fn SET_PRVALUE(x: SEXP, v: SEXP);

    pub fn SET_STRING_ELT(x: SEXP, i: R_xlen_t, v: SEXP);

    pub fn SET_LOGICAL_ELT(x: SEXP, i: R_xlen_t, v: std::ffi::c_int);

    pub fn SET_INTEGER_ELT(x: SEXP, i: R_xlen_t, v: std::ffi::c_int);

    pub fn SET_REAL_ELT(x: SEXP, i: R_xlen_t, v: f64);

    pub fn SET_COMPLEX_ELT(x: SEXP, i: R_xlen_t, v: Rcomplex);

    pub fn SET_TAG(x: SEXP, y: SEXP);

    pub fn SET_TYPEOF(x: SEXP, v: std::ffi::c_int);

    pub fn SET_VECTOR_ELT(x: SEXP, i: R_xlen_t, v: SEXP) -> SEXP;

    pub fn STRING_ELT(x: SEXP, i: R_xlen_t) -> SEXP;

    pub fn TAG(e: SEXP) -> SEXP;

    pub fn TYPEOF(x: SEXP) -> std::ffi::c_int;

    pub fn VECTOR_ELT(x: SEXP, i: R_xlen_t) -> SEXP;

    pub fn R_GE_getVersion() -> std::ffi::c_int;

    pub fn GEcurrentDevice() -> pGEDevDesc;

    pub fn GEinitDisplayList(dd: pGEDevDesc);

    pub fn ENVFLAGS(x: SEXP) -> std::ffi::c_int;

    pub fn SET_ENVFLAGS(x: SEXP, v: std::ffi::c_int);

    pub fn R_LockEnvironment(env: SEXP, bindings: Rboolean);

    pub fn R_EnvironmentIsLocked(env: SEXP) -> Rboolean;

    pub fn CLOENV(x: SEXP) -> SEXP;

    pub fn BODY(x: SEXP) -> SEXP;

    pub fn SET_BODY(x: SEXP, v: SEXP);

    pub fn R_ClosureExpr(x: SEXP) -> SEXP;

    pub fn Rf_PrintValue(x: SEXP);

    pub fn R_PromiseExpr(p: SEXP) -> SEXP;

    pub fn R_BytecodeExpr(e: SEXP) -> SEXP;

    pub fn Rf_installChar(x: SEXP) -> SEXP;

    /// R >= 4.2.0
    pub fn R_existsVarInFrame(rho: SEXP, symbol: SEXP) -> Rboolean;

    pub fn R_ActiveBindingFunction(sym: SEXP, env: SEXP) -> SEXP;

    pub fn R_LockBinding(sym: SEXP, env: SEXP) -> SEXP;

    pub fn R_unLockBinding(sym: SEXP, env: SEXP) -> SEXP;

    pub fn R_BindingIsLocked(sym: SEXP, env: SEXP) -> Rboolean;

    // -----------------------------------------------------------------------------------
    // Unix

    /// NOTE: `R_checkActivity()` doesn't really return a void pointer, it returns
    /// a `*fd_set`. But because we never introspect these values directly and they're
    /// always passed around in R as pointers, it suffices to just use void pointers.
    #[cfg(target_family = "unix")]
    pub fn R_checkActivity(usec: i32, ignore_stdin: i32) -> *const std::ffi::c_void;

    /// NOTE: `R_runHandlers()` doesn't really take void pointers. But because we never
    /// introspect these values directly and they're always passed around in R as
    /// pointers, it suffices to just use void pointers.
    #[cfg(target_family = "unix")]
    pub fn R_runHandlers(handlers: *const std::ffi::c_void, fdset: *const std::ffi::c_void);

    // -----------------------------------------------------------------------------------
    // Windows

    #[cfg(target_family = "windows")]
    pub fn cmdlineoptions(ac: i32, av: *mut *mut std::ffi::c_char);

    #[cfg(target_family = "windows")]
    pub fn readconsolecfg();

    /// R >= 4.2.0
    #[cfg(target_family = "windows")]
    pub fn R_DefParamsEx(Rp: Rstart, RstartVersion: i32);

    #[cfg(target_family = "windows")]
    pub fn R_SetParams(Rp: Rstart);

    /// Get R_HOME from the environment or the registry
    ///
    /// Checks:
    /// - C `R_HOME` env var
    /// - Windows API `R_HOME` environment space
    /// - Current user registry
    /// - Local machine registry
    ///
    /// Probably returns a system encoded result?
    /// So needs to be converted to UTF-8.
    ///
    /// https://github.com/wch/r-source/blob/55cd975c538ad5a086c2085ccb6a3037d5a0cb9a/src/gnuwin32/rhome.c#L152
    #[cfg(target_family = "windows")]
    pub fn get_R_HOME() -> *mut std::ffi::c_char;

    /// Get user home directory
    ///
    /// Checks:
    /// - C `R_USER` env var
    /// - C `USER` env var
    /// - `Documents` folder, if one exists, through `ShellGetPersonalDirectory()`
    /// - `HOMEDRIVE` + `HOMEPATH`
    /// - Current directory through `GetCurrentDirectory()`
    ///
    /// Probably returns a system encoded result?
    /// So needs to be converted to UTF-8.
    ///
    /// https://github.com/wch/r-source/blob/55cd975c538ad5a086c2085ccb6a3037d5a0cb9a/src/gnuwin32/shext.c#L55
    #[cfg(target_family = "windows")]
    pub fn getRUser() -> *mut std::ffi::c_char;

    // In theory we should call these, but they are very new, roughly R 4.3.0.
    // It isn't super harmful if we don't free these.
    // https://github.com/wch/r-source/commit/9210c59281e7ab93acff9f692c31b83d07a506a6
    // pub fn freeRUser(s: *mut std::ffi::c_char);
    // pub fn free_R_HOME(s: *mut std::ffi::c_char);
}

functions_variadic::generate! {
    pub fn Rf_error(arg1: *const std::ffi::c_char, ...) -> !;

    pub fn Rf_errorcall(arg1: SEXP, arg2: *const std::ffi::c_char, ...) -> !;

    pub fn Rprintf(x: *const std::ffi::c_char, ...);
}

constant_globals::generate! {
    #[doc = "IEEE NaN"]
    #[default = 0.0]
    pub static R_NaN: f64;

    #[doc = "IEEE Inf"]
    #[default = 0.0]
    pub static R_PosInf: f64;

    #[doc = "IEEE -Inf"]
    #[default = 0.0]
    pub static R_NegInf: f64;

    #[doc = "NA_REAL: IEEE"]
    #[default = 0.0]
    pub static R_NaReal: f64;

    #[doc = "NA_INTEGER:= INT_MIN currently"]
    #[default = 0]
    pub static R_NaInt: std::ffi::c_int;

    #[doc = "The \"global\" environment"]
    #[default = std::ptr::null_mut()]
    pub static R_GlobalEnv: SEXP;

    #[doc = "An empty environment at the root of the\nenvironment tree"]
    #[default = std::ptr::null_mut()]
    pub static R_EmptyEnv: SEXP;

    #[doc = "The base environment; formerly R_NilValue"]
    #[default = std::ptr::null_mut()]
    pub static R_BaseEnv: SEXP;

    #[doc = "The (fake) namespace for base"]
    #[default = std::ptr::null_mut()]
    pub static R_BaseNamespace: SEXP;

    #[doc = "Registry for registered namespaces"]
    #[default = std::ptr::null_mut()]
    pub static R_NamespaceRegistry: SEXP;

    #[doc = "The nil object"]
    #[default = std::ptr::null_mut()]
    pub static R_NilValue: SEXP;

    #[doc = "Unbound marker"]
    #[default = std::ptr::null_mut()]
    pub static R_UnboundValue: SEXP;

    #[doc = "Missing argument marker"]
    #[default = std::ptr::null_mut()]
    pub static R_MissingArg: SEXP;

    #[doc = "To be found in BC interp. state\n(marker)"]
    #[default = std::ptr::null_mut()]
    pub static R_InBCInterpreter: SEXP;

    #[doc = "Use current expression (marker)"]
    #[default = std::ptr::null_mut()]
    pub static R_CurrentExpression: SEXP;

    #[doc = "Marker for restarted function calls"]
    #[default = std::ptr::null_mut()]
    pub static R_RestartToken: SEXP;

    #[doc = "\"as.character\""]
    #[default = std::ptr::null_mut()]
    pub static R_AsCharacterSymbol: SEXP;

    #[doc = "\"@\""]
    #[default = std::ptr::null_mut()]
    pub static R_AtsignSymbol: SEXP;

    #[doc = "\"base\""]
    #[default = std::ptr::null_mut()]
    pub static R_BaseSymbol: SEXP;

    #[doc = "\"{\""]
    #[default = std::ptr::null_mut()]
    pub static R_BraceSymbol: SEXP;

    #[doc = "\"\\[\\[\""]
    #[default = std::ptr::null_mut()]
    pub static R_Bracket2Symbol: SEXP;

    #[doc = "\"\\[\""]
    #[default = std::ptr::null_mut()]
    pub static R_BracketSymbol: SEXP;

    #[doc = "\"class\""]
    #[default = std::ptr::null_mut()]
    pub static R_ClassSymbol: SEXP;

    #[doc = "\".Device\""]
    #[default = std::ptr::null_mut()]
    pub static R_DeviceSymbol: SEXP;

    #[doc = "\"dimnames\""]
    #[default = std::ptr::null_mut()]
    pub static R_DimNamesSymbol: SEXP;

    #[doc = "\"dim\""]
    #[default = std::ptr::null_mut()]
    pub static R_DimSymbol: SEXP;

    #[doc = "\"$\""]
    #[default = std::ptr::null_mut()]
    pub static R_DollarSymbol: SEXP;

    #[doc = "\"...\""]
    #[default = std::ptr::null_mut()]
    pub static R_DotsSymbol: SEXP;

    #[doc = "\"::\""]
    #[default = std::ptr::null_mut()]
    pub static R_DoubleColonSymbol: SEXP;

    #[doc = "\"drop\""]
    #[default = std::ptr::null_mut()]
    pub static R_DropSymbol: SEXP;

    #[doc = "\"eval\""]
    #[default = std::ptr::null_mut()]
    pub static R_EvalSymbol: SEXP;

    #[doc = "\"function\""]
    #[default = std::ptr::null_mut()]
    pub static R_FunctionSymbol: SEXP;

    #[doc = "\".Last.value\""]
    #[default = std::ptr::null_mut()]
    pub static R_LastvalueSymbol: SEXP;

    #[doc = "\"levels\""]
    #[default = std::ptr::null_mut()]
    pub static R_LevelsSymbol: SEXP;

    #[doc = "\"mode\""]
    #[default = std::ptr::null_mut()]
    pub static R_ModeSymbol: SEXP;

    #[doc = "\"na.rm\""]
    #[default = std::ptr::null_mut()]
    pub static R_NaRmSymbol: SEXP;

    #[doc = "\"name\""]
    #[default = std::ptr::null_mut()]
    pub static R_NameSymbol: SEXP;

    #[doc = "\"names\""]
    #[default = std::ptr::null_mut()]
    pub static R_NamesSymbol: SEXP;

    #[doc = "\".__NAMESPACE__.\""]
    #[default = std::ptr::null_mut()]
    pub static R_NamespaceEnvSymbol: SEXP;

    #[doc = "\"package\""]
    #[default = std::ptr::null_mut()]
    pub static R_PackageSymbol: SEXP;

    #[doc = "\"previous\""]
    #[default = std::ptr::null_mut()]
    pub static R_PreviousSymbol: SEXP;

    #[doc = "\"quote\""]
    #[default = std::ptr::null_mut()]
    pub static R_QuoteSymbol: SEXP;

    #[doc = "\"row.names\""]
    #[default = std::ptr::null_mut()]
    pub static R_RowNamesSymbol: SEXP;

    #[doc = "\".Random.seed\""]
    #[default = std::ptr::null_mut()]
    pub static R_SeedsSymbol: SEXP;

    #[doc = "\"sort.list\""]
    #[default = std::ptr::null_mut()]
    pub static R_SortListSymbol: SEXP;

    #[doc = "\"source\""]
    #[default = std::ptr::null_mut()]
    pub static R_SourceSymbol: SEXP;

    #[doc = "\"spec\""]
    #[default = std::ptr::null_mut()]
    pub static R_SpecSymbol: SEXP;

    #[doc = "\":::\""]
    #[default = std::ptr::null_mut()]
    pub static R_TripleColonSymbol: SEXP;

    #[doc = "\"tsp\""]
    #[default = std::ptr::null_mut()]
    pub static R_TspSymbol: SEXP;

    #[doc = "\".defined\""]
    #[default = std::ptr::null_mut()]
    pub static R_dot_defined: SEXP;

    #[doc = "\".Method\""]
    #[default = std::ptr::null_mut()]
    pub static R_dot_Method: SEXP;

    #[doc = "\".packageName\""]
    #[default = std::ptr::null_mut()]
    pub static R_dot_packageName: SEXP;

    #[doc = "\".target\""]
    #[default = std::ptr::null_mut()]
    pub static R_dot_target: SEXP;

    #[doc = "\".Generic\""]
    #[default = std::ptr::null_mut()]
    pub static R_dot_Generic: SEXP;

    #[doc = "NA_STRING as a CHARSXP"]
    #[default = std::ptr::null_mut()]
    pub static R_NaString: SEXP;

    #[doc = "\"\" as a CHARSXP"]
    #[default = std::ptr::null_mut()]
    pub static R_BlankString: SEXP;

    #[doc = "\"\" as a STRSXP"]
    #[default = std::ptr::null_mut()]
    pub static R_BlankScalarString: SEXP;
}

mutable_globals::generate! {
    pub static mut R_Interactive: Rboolean;

    pub static mut R_interrupts_pending: std::ffi::c_int;

    pub static mut R_interrupts_suspended: Rboolean;

    /// Special declaration for this global variable
    ///
    /// I don't fully understand this!
    ///
    /// This is exposed in Rinterface.h, which is not available on Windows:
    /// https://github.com/wch/r-source/blob/459492bc14ad5a3ff735d90a70ad71f6d5fe9faa/src/include/Rinterface.h#L176
    /// But is defined as a global variable in main.c, so presumably that is what RStudio is yanking out
    /// https://github.com/wch/r-source/blob/459492bc14ad5a3ff735d90a70ad71f6d5fe9faa/src/main/main.c#L729
    /// It controls whether R level signal handlers are set up, which presumably we don't want
    /// https://github.com/wch/r-source/blob/459492bc14ad5a3ff735d90a70ad71f6d5fe9faa/src/main/main.c#L1047
    /// RStudio sets this, and I think they access it by using this dllimport
    /// https://github.com/rstudio/rstudio/blob/07ef754fc9f27d41b100bb40d83ec3ddf485b47b/src/cpp/r/include/r/RInterface.hpp#L40
    pub static mut R_SignalHandlers: std::ffi::c_int;

    pub static mut R_ParseError: std::ffi::c_int;

    /// 256 comes from R's `PARSE_ERROR_SIZE` define
    pub static mut R_ParseErrorMsg: [std::ffi::c_char; 256usize];

    pub static mut R_DirtyImage: std::ffi::c_int;

    pub static mut R_CStackLimit: usize;

    pub static mut R_Srcref: SEXP;

    // -----------------------------------------------------------------------------------
    // Unix

    #[cfg(target_family = "unix")]
    pub static mut R_running_as_main_program: std::ffi::c_int;

    #[cfg(target_family = "unix")]
    pub static mut R_InputHandlers: *const std::ffi::c_void;

    #[cfg(target_family = "unix")]
    pub static mut R_Consolefile: *mut libc::FILE;

    #[cfg(target_family = "unix")]
    pub static mut R_Outputfile: *mut libc::FILE;

    #[cfg(target_family = "unix")]
    pub static mut R_wait_usec: i32;

    #[cfg(target_family = "unix")]
    pub static mut R_PolledEvents: Option<unsafe extern "C" fn()>;

    #[cfg(target_family = "unix")]
    pub static mut ptr_R_WriteConsole: Option<
        unsafe extern "C" fn(arg1: *const std::ffi::c_char, arg2: std::ffi::c_int),
    >;

    #[cfg(target_family = "unix")]
    pub static mut ptr_R_WriteConsoleEx: Option<
        unsafe extern "C" fn(
            arg1: *const std::ffi::c_char,
            arg2: std::ffi::c_int,
            arg3: std::ffi::c_int,
        ),
    >;

    #[cfg(target_family = "unix")]
    pub static mut ptr_R_ReadConsole: Option<
        unsafe extern "C" fn(
            arg1: *const std::ffi::c_char,
            arg2: *mut std::ffi::c_uchar,
            arg3: std::ffi::c_int,
            arg4: std::ffi::c_int,
        ) -> std::ffi::c_int,
    >;

    #[cfg(target_family = "unix")]
    pub static mut ptr_R_ShowMessage: Option<unsafe extern "C" fn(arg1: *const std::ffi::c_char)>;

    #[cfg(target_family = "unix")]
    pub static mut ptr_R_Busy: Option<unsafe extern "C" fn(arg1: std::ffi::c_int)>;

    // -----------------------------------------------------------------------------------
    // Windows

    #[cfg(target_family = "windows")]
    pub static mut UserBreak: Rboolean;

    /// The codepage that R thinks it should be using for Windows.
    /// Should map to `winsafe::kernel::co::CP`.
    #[cfg(target_family = "windows")]
    pub static mut localeCP: std::ffi::c_uint;
}
