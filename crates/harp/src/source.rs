//
// source.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use libr::SEXP;

use crate::environment::R_ENVS;
use crate::exec::RFunction;
use crate::exec::RFunctionExt;

pub fn source(file: &str) -> crate::Result<()> {
    source_in(file, R_ENVS.base)
}

pub fn source_in(file: &str, env: SEXP) -> crate::Result<()> {
    RFunction::new("base", "sys.source")
        .param("file", file)
        .param("envir", env)
        .call()?;

    Ok(())
}

pub fn source_str(code: &str) -> crate::Result<()> {
    source_str_in(code, R_ENVS.base)
}

pub fn source_str_in(code: &str, env: impl Into<SEXP>) -> crate::Result<()> {
    let exprs = harp::parse_exprs(code)?;
    source_exprs_in(exprs, env)?;
    Ok(())
}

pub fn source_exprs(exprs: impl Into<SEXP>) -> crate::Result<()> {
    source_exprs_in(exprs, R_ENVS.base)
}

pub fn source_exprs_in(exprs: impl Into<SEXP>, env: impl Into<SEXP>) -> crate::Result<()> {
    let exprs = exprs.into();
    let env = env.into();

    // `exprs` is an EXPRSXP and doesn't need to be quoted when passed as
    // literal argument. Only the R-level `eval()` function evaluates expression
    // vectors.
    RFunction::new("base", "source")
        .param("exprs", exprs)
        .param("local", env)
        .call()?;

    Ok(())
}
