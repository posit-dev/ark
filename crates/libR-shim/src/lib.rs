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

pub use libR_sys::pDevDesc;
pub use libR_sys::pGEcontext;
pub use libR_sys::GEcurrentDevice;
pub use libR_sys::GEinitDisplayList;
pub use libR_sys::R_GE_getVersion;
pub use libR_sys::R_xlen_t;
pub use libR_sys::Rboolean;
pub use libR_sys::Rboolean_FALSE;
pub use libR_sys::Rboolean_TRUE;
pub use libR_sys::Rcomplex;
pub use libR_sys::Rf_error;
pub use libR_sys::Rf_errorcall;
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
