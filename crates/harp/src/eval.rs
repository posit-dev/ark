//
// eval.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

use crate::environment::R_ENVS;
use crate::error::Error;
use crate::object::RObject;

#[derive(Clone)]
pub struct RParseEvalOptions {
    pub forbid_function_calls: bool,
    pub env: RObject,
}

impl Default for RParseEvalOptions {
    fn default() -> Self {
        Self {
            forbid_function_calls: false,
            env: RObject::view(R_ENVS.global),
        }
    }
}

pub fn parse_eval0(code: &str, env: impl Into<RObject>) -> harp::Result<RObject> {
    harp::parse_eval(code, RParseEvalOptions {
        env: env.into(),
        ..Default::default()
    })
}

pub fn parse_eval_global(code: &str) -> harp::Result<RObject> {
    harp::parse_eval(code, RParseEvalOptions {
        env: R_ENVS.global.into(),
        ..Default::default()
    })
}

pub fn parse_eval_base(code: &str) -> harp::Result<RObject> {
    harp::parse_eval(code, RParseEvalOptions {
        env: R_ENVS.base.into(),
        ..Default::default()
    })
}

pub fn parse_eval(code: &str, options: RParseEvalOptions) -> harp::Result<RObject> {
    // Forbid certain kinds of evaluation if requested.
    if options.forbid_function_calls && code.find('(').is_some() {
        return Err(Error::UnsafeEvaluationError(code.to_string()));
    }

    let exprs = harp::parse_exprs(code)?;

    // Evaluate each expression in turn and return the last one
    let mut value = RObject::null();

    for i in 0..exprs.length() {
        let expr = harp::list_get(exprs.sexp, i);
        value = harp::try_eval_silent(expr, options.env.sexp)?;
    }

    Ok(value)
}
