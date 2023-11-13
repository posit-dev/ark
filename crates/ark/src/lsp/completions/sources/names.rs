//
// names.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::Result;
use harp::error::Error;
use harp::eval::r_parse_eval;
use harp::eval::RParseEvalOptions;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::completions::completion_item::completion_item_from_data_variable;

pub(super) fn completions_from_evaluated_object_names(
    name: &str,
    enquote: bool,
) -> Result<Option<Vec<CompletionItem>>> {
    log::info!("completions_from_evaluated_object_names({name:?})");

    let options = RParseEvalOptions {
        forbid_function_calls: true,
    };

    // Try to evaluate the object
    let object = unsafe { r_parse_eval(name, options) };

    // If the user is writing pseudocode, this object might not exist yet,
    // in which case we just want to ignore the error from trying to evaluate it
    // and just provide typical completions.
    // If we get an `UnsafeEvaluationError` here from setting
    // `forbid_function_calls`, we don't even log that one, as that is
    // expected to happen with complex inputs.
    let object = match object {
        Ok(object) => object,
        Err(err) => match err {
            Error::UnsafeEvaluationError(_) => return Ok(None),
            _ => {
                log::info!(
                    "completions_from_evaluated_object_names(): Failed to evaluate first argument: {err}"
                );
                return Ok(None);
            },
        },
    };

    Ok(Some(completions_from_object_names(object, name, enquote)?))
}

pub(super) fn completions_from_object_names(
    object: RObject,
    name: &str,
    enquote: bool,
) -> Result<Vec<CompletionItem>> {
    log::info!("completions_from_object_names({object:?})");

    let mut completions = vec![];

    unsafe {
        let variable_names = RFunction::new("base", "names")
            .add(object)
            .call()?
            .to::<Vec<String>>()?;

        for variable_name in variable_names {
            match completion_item_from_data_variable(&variable_name, name, enquote) {
                Ok(item) => completions.push(item),
                Err(err) => log::error!("{err:?}"),
            }
        }
    }

    Ok(completions)
}
