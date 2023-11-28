//
// extractor.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::Result;
use harp::eval::r_parse_eval;
use harp::eval::RParseEvalOptions;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::r_symbol;
use harp::utils::r_env_has;
use harp::utils::r_typeof;
use libR_sys::STRSXP;
use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::completions::completion_item::completion_item_from_data_variable;
use crate::lsp::completions::sources::utils::set_sort_text_by_first_appearance;
use crate::lsp::document_context::DocumentContext;

pub fn completions_from_dollar(context: &DocumentContext) -> Result<Option<Vec<CompletionItem>>> {
    completions_from_extractor(context, "$", ".DollarNames")
}

pub fn completions_from_at(context: &DocumentContext) -> Result<Option<Vec<CompletionItem>>> {
    completions_from_extractor(context, "@", ".AtNames")
}

fn completions_from_extractor(
    context: &DocumentContext,
    token: &str,
    fun: &str,
) -> Result<Option<Vec<CompletionItem>>> {
    log::info!("completions_from_extractor()");

    let mut node = context.node;

    if node.kind() != token {
        return Ok(None);
    }

    if !node.is_named() {
        // If we are on the literal operator, look up one level to find the
        // parent. We have to do this because `DocumentContext` considers all
        // nodes, not just named ones.
        match node.parent() {
            Some(parent) => node = parent,
            None => return Ok(None),
        }
        if node.kind() != token {
            return Ok(None);
        }
    }

    let mut completions: Vec<CompletionItem> = vec![];

    let Some(child) = node.child(0) else {
        return Ok(Some(completions));
    };

    let text = child.utf8_text(context.source.as_bytes())?;

    completions.append(&mut completions_from_extractor_helper(&text, fun)?);

    Ok(Some(completions))
}

fn completions_from_extractor_helper(object: &str, fun: &str) -> Result<Vec<CompletionItem>> {
    log::info!("completions_from_extractor_helper({object:?}, {fun:?})");

    const ENQUOTE: bool = false;

    let mut completions = vec![];

    unsafe {
        let env_utils = RFunction::new("base", "asNamespace").add("utils").call()?;
        let sym = r_symbol!(fun);

        if !r_env_has(*env_utils, sym) {
            // We'd like to generate these completions, but not a new enough version of R
            return Ok(completions);
        }

        let value = r_parse_eval(object, RParseEvalOptions {
            forbid_function_calls: true,
            ..Default::default()
        })?;

        let names = RFunction::new("utils", fun).add(value).call()?;

        if r_typeof(*names) != STRSXP {
            // Could come from a malformed user supplied S3 method
            return Ok(completions);
        }

        let names = names.to::<Vec<String>>()?;

        for name in names {
            match completion_item_from_data_variable(&name, object, ENQUOTE) {
                Ok(item) => completions.push(item),
                Err(err) => log::error!("{err:?}"),
            }
        }
    }

    // People typically expect that `$` and `@` completions are returned in
    // the same order as in the underlying object.
    set_sort_text_by_first_appearance(&mut completions);

    Ok(completions)
}
