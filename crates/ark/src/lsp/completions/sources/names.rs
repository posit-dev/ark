//
// names.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::Result;
use harp::eval::r_parse_eval;
use harp::eval::RParseEvalOptions;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::completions::completion_item::completion_item_from_data_variable;

pub(super) fn completions_from_object_names(
    object: &str,
    enquote: bool,
) -> Result<Vec<CompletionItem>> {
    log::info!("completions_from_object_names({object:?})");

    let mut completions = vec![];

    unsafe {
        let value = r_parse_eval(object, RParseEvalOptions {
            forbid_function_calls: true,
        })?;

        let names = RFunction::new("base", "names")
            .add(value)
            .call()?
            .to::<Vec<String>>()?;

        for name in names {
            match completion_item_from_data_variable(&name, object, enquote) {
                Ok(item) => completions.push(item),
                Err(err) => log::error!("{err:?}"),
            }
        }
    }

    Ok(completions)
}
