//
// lib.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(improper_ctypes)]

pub use libR_sys::cetype_t_CE_UTF8;
pub use libR_sys::pDevDesc;
pub use libR_sys::pGEcontext;
pub use libR_sys::setup_Rmainloop;
pub use libR_sys::vmaxget;
pub use libR_sys::vmaxset;
pub use libR_sys::GEcurrentDevice;
pub use libR_sys::GEinitDisplayList;
pub use libR_sys::ParseStatus;
pub use libR_sys::ParseStatus_PARSE_ERROR;
pub use libR_sys::ParseStatus_PARSE_INCOMPLETE;
pub use libR_sys::ParseStatus_PARSE_NULL;
pub use libR_sys::ParseStatus_PARSE_OK;
pub use libR_sys::R_BindingIsActive;
pub use libR_sys::R_CallMethodDef;
pub use libR_sys::R_CheckStack;
pub use libR_sys::R_CheckStack2;
pub use libR_sys::R_ExternalPtrAddr;
pub use libR_sys::R_GE_getVersion;
pub use libR_sys::R_IsNA;
pub use libR_sys::R_IsNamespaceEnv;
pub use libR_sys::R_IsPackageEnv;
pub use libR_sys::R_MakeExternalPtr;
pub use libR_sys::R_NamespaceEnvSpec;
pub use libR_sys::R_PackageEnvName;
pub use libR_sys::R_ParseVector;
pub use libR_sys::R_PreserveObject;
pub use libR_sys::R_RunPendingFinalizers;
pub use libR_sys::R_ToplevelExec;
pub use libR_sys::R_altrep_data1;
pub use libR_sys::R_altrep_data2;
pub use libR_sys::R_curErrorBuf;
pub use libR_sys::R_do_slot;
pub use libR_sys::R_existsVarInFrame;
pub use libR_sys::R_getEmbeddingDllInfo;
pub use libR_sys::R_lsInternal;
pub use libR_sys::R_registerRoutines;
pub use libR_sys::R_tryCatch;
pub use libR_sys::R_tryEvalSilent;
pub use libR_sys::R_xlen_t;
pub use libR_sys::Rboolean;
pub use libR_sys::Rboolean_TRUE;
pub use libR_sys::Rcomplex;
pub use libR_sys::Rf_GetOption1;
pub use libR_sys::Rf_ScalarInteger;
pub use libR_sys::Rf_ScalarLogical;
pub use libR_sys::Rf_ScalarReal;
pub use libR_sys::Rf_ScalarString;
pub use libR_sys::Rf_allocVector;
pub use libR_sys::Rf_asInteger;
pub use libR_sys::Rf_coerceVector;
pub use libR_sys::Rf_cons;
pub use libR_sys::Rf_defineVar;
pub use libR_sys::Rf_error;
pub use libR_sys::Rf_errorcall;
pub use libR_sys::Rf_eval;
pub use libR_sys::Rf_findVar;
pub use libR_sys::Rf_findVarInFrame;
pub use libR_sys::Rf_getAttrib;
pub use libR_sys::Rf_inherits;
pub use libR_sys::Rf_initialize_R;
pub use libR_sys::Rf_install;
pub use libR_sys::Rf_isFunction;
pub use libR_sys::Rf_isInteger;
pub use libR_sys::Rf_isMatrix;
pub use libR_sys::Rf_isNumeric;
pub use libR_sys::Rf_isString;
pub use libR_sys::Rf_lang1;
pub use libR_sys::Rf_lang2;
pub use libR_sys::Rf_lang3;
pub use libR_sys::Rf_lang4;
pub use libR_sys::Rf_lcons;
pub use libR_sys::Rf_length;
pub use libR_sys::Rf_mkCharLenCE;
pub use libR_sys::Rf_mkString;
pub use libR_sys::Rf_onintr;
pub use libR_sys::Rf_protect;
pub use libR_sys::Rf_setAttrib;
pub use libR_sys::Rf_setVar;
pub use libR_sys::Rf_translateCharUTF8;
pub use libR_sys::Rf_type2char;
pub use libR_sys::Rf_unprotect;
pub use libR_sys::Rf_xlength;
pub use libR_sys::ALTREP;
pub use libR_sys::ALTREP_CLASS;
pub use libR_sys::ATTRIB;
pub use libR_sys::BUILTINSXP;
pub use libR_sys::CADDDR;
pub use libR_sys::CADDR;
pub use libR_sys::CADR;
pub use libR_sys::CAR;
pub use libR_sys::CDDDR;
pub use libR_sys::CDDR;
pub use libR_sys::CDR;
pub use libR_sys::CHARSXP;
pub use libR_sys::CLOSXP;
pub use libR_sys::COMPLEX_ELT;
pub use libR_sys::CPLXSXP;
pub use libR_sys::DATAPTR;
pub use libR_sys::ENCLOS;
pub use libR_sys::ENVSXP;
pub use libR_sys::EXPRSXP;
pub use libR_sys::FILE;
pub use libR_sys::FORMALS;
pub use libR_sys::FRAME;
pub use libR_sys::HASHTAB;
pub use libR_sys::INTEGER_ELT;
pub use libR_sys::INTSXP;
pub use libR_sys::LANGSXP;
pub use libR_sys::LGLSXP;
pub use libR_sys::LISTSXP;
pub use libR_sys::LOGICAL;
pub use libR_sys::LOGICAL_ELT;
pub use libR_sys::NILSXP;
pub use libR_sys::PRCODE;
pub use libR_sys::PRENV;
pub use libR_sys::PRINTNAME;
pub use libR_sys::PROMSXP;
pub use libR_sys::PRVALUE;
pub use libR_sys::RAW;
pub use libR_sys::RAWSXP;
pub use libR_sys::RAW_ELT;
pub use libR_sys::RDEBUG;
pub use libR_sys::REAL;
pub use libR_sys::REALSXP;
pub use libR_sys::REAL_ELT;
pub use libR_sys::R_CHAR;
pub use libR_sys::SETCAR;
pub use libR_sys::SETCDR;
pub use libR_sys::SET_PRVALUE;
pub use libR_sys::SET_STRING_ELT;
pub use libR_sys::SET_TAG;
pub use libR_sys::SET_TYPEOF;
pub use libR_sys::SET_VECTOR_ELT;
pub use libR_sys::SEXP;
pub use libR_sys::SEXPREC;
pub use libR_sys::SPECIALSXP;
pub use libR_sys::STRING_ELT;
pub use libR_sys::STRSXP;
pub use libR_sys::SYMSXP;
pub use libR_sys::TAG;
pub use libR_sys::TYPEOF;
pub use libR_sys::VECSXP;
pub use libR_sys::VECTOR_ELT;
pub use libR_sys::XLENGTH;

// Global variables not exported by libR_sys, but we need them as an IDE
#[link(name = "R", kind = "dylib")]
extern "C" {
    pub static mut R_SignalHandlers: ::std::os::raw::c_int;
}

// Global variables exported by libR_sys, but without the `#[link]` attribute,
// so they don't work on Windows
#[link(name = "R", kind = "dylib")]
extern "C" {
    pub static mut R_interrupts_suspended: Rboolean;
    pub static mut R_interrupts_pending: ::std::os::raw::c_int;

    #[doc = "IEEE NaN"]
    pub static mut R_NaN: f64;
    #[doc = "IEEE Inf"]
    pub static mut R_PosInf: f64;
    #[doc = "IEEE -Inf"]
    pub static mut R_NegInf: f64;
    #[doc = "NA_REAL: IEEE"]
    pub static mut R_NaReal: f64;
    #[doc = "NA_INTEGER:= INT_MIN currently"]
    pub static mut R_NaInt: ::std::os::raw::c_int;

    #[doc = "C stack limit"]
    pub static mut R_CStackLimit: usize;

    #[doc = "The \"global\" environment"]
    pub static mut R_GlobalEnv: SEXP;
    #[doc = "An empty environment at the root of the\nenvironment tree"]
    pub static mut R_EmptyEnv: SEXP;
    #[doc = "The base environment; formerly R_NilValue"]
    pub static mut R_BaseEnv: SEXP;
    #[doc = "The (fake) namespace for base"]
    pub static mut R_BaseNamespace: SEXP;
    #[doc = "Registry for registered namespaces"]
    pub static mut R_NamespaceRegistry: SEXP;
    #[doc = "Current srcref, for debuggers"]
    pub static mut R_Srcref: SEXP;
    #[doc = "The nil object"]
    pub static mut R_NilValue: SEXP;
    #[doc = "Unbound marker"]
    pub static mut R_UnboundValue: SEXP;
    #[doc = "Missing argument marker"]
    pub static mut R_MissingArg: SEXP;
    #[doc = "To be found in BC interp. state\n(marker)"]
    pub static mut R_InBCInterpreter: SEXP;
    #[doc = "Use current expression (marker)"]
    pub static mut R_CurrentExpression: SEXP;
    #[doc = "Marker for restarted function calls"]
    pub static mut R_RestartToken: SEXP;
    #[doc = "\"as.character\""]
    pub static mut R_AsCharacterSymbol: SEXP;
    #[doc = "\"@\""]
    pub static mut R_AtsignSymbol: SEXP;
    #[doc = "<-- backcompatible version of:"]
    pub static mut R_baseSymbol: SEXP;
    #[doc = "\"base\""]
    pub static mut R_BaseSymbol: SEXP;
    #[doc = "\"{\""]
    pub static mut R_BraceSymbol: SEXP;
    #[doc = "\"\\[\\[\""]
    pub static mut R_Bracket2Symbol: SEXP;
    #[doc = "\"\\[\""]
    pub static mut R_BracketSymbol: SEXP;
    #[doc = "\"class\""]
    pub static mut R_ClassSymbol: SEXP;
    #[doc = "\".Device\""]
    pub static mut R_DeviceSymbol: SEXP;
    #[doc = "\"dimnames\""]
    pub static mut R_DimNamesSymbol: SEXP;
    #[doc = "\"dim\""]
    pub static mut R_DimSymbol: SEXP;
    #[doc = "\"$\""]
    pub static mut R_DollarSymbol: SEXP;
    #[doc = "\"...\""]
    pub static mut R_DotsSymbol: SEXP;
    #[doc = "\"::\""]
    pub static mut R_DoubleColonSymbol: SEXP;
    #[doc = "\"drop\""]
    pub static mut R_DropSymbol: SEXP;
    #[doc = "\"eval\""]
    pub static mut R_EvalSymbol: SEXP;
    #[doc = "\"function\""]
    pub static mut R_FunctionSymbol: SEXP;
    #[doc = "\".Last.value\""]
    pub static mut R_LastvalueSymbol: SEXP;
    #[doc = "\"levels\""]
    pub static mut R_LevelsSymbol: SEXP;
    #[doc = "\"mode\""]
    pub static mut R_ModeSymbol: SEXP;
    #[doc = "\"na.rm\""]
    pub static mut R_NaRmSymbol: SEXP;
    #[doc = "\"name\""]
    pub static mut R_NameSymbol: SEXP;
    #[doc = "\"names\""]
    pub static mut R_NamesSymbol: SEXP;
    #[doc = "\".__NAMESPACE__.\""]
    pub static mut R_NamespaceEnvSymbol: SEXP;
    #[doc = "\"package\""]
    pub static mut R_PackageSymbol: SEXP;
    #[doc = "\"previous\""]
    pub static mut R_PreviousSymbol: SEXP;
    #[doc = "\"quote\""]
    pub static mut R_QuoteSymbol: SEXP;
    #[doc = "\"row.names\""]
    pub static mut R_RowNamesSymbol: SEXP;
    #[doc = "\".Random.seed\""]
    pub static mut R_SeedsSymbol: SEXP;
    #[doc = "\"sort.list\""]
    pub static mut R_SortListSymbol: SEXP;
    #[doc = "\"source\""]
    pub static mut R_SourceSymbol: SEXP;
    #[doc = "\"spec\""]
    pub static mut R_SpecSymbol: SEXP;
    #[doc = "\":::\""]
    pub static mut R_TripleColonSymbol: SEXP;
    #[doc = "\"tsp\""]
    pub static mut R_TspSymbol: SEXP;
    #[doc = "\".defined\""]
    pub static mut R_dot_defined: SEXP;
    #[doc = "\".Method\""]
    pub static mut R_dot_Method: SEXP;
    #[doc = "\".packageName\""]
    pub static mut R_dot_packageName: SEXP;
    #[doc = "\".target\""]
    pub static mut R_dot_target: SEXP;
    #[doc = "\".Generic\""]
    pub static mut R_dot_Generic: SEXP;
    #[doc = "NA_STRING as a CHARSXP"]
    pub static mut R_NaString: SEXP;
    #[doc = "\"\" as a CHARSXP"]
    pub static mut R_BlankString: SEXP;
    #[doc = "\"\" as a STRSXP"]
    pub static mut R_BlankScalarString: SEXP;
}
