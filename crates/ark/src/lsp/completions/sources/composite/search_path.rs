//
// search_path.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::Result;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::utils::r_env_is_pkg_env;
use harp::utils::r_pkg_env_name;
use harp::vector::CharacterVector;
use harp::vector::Vector;
use harp::RObject;
use libr::R_EmptyEnv;
use libr::R_GlobalEnv;
use libr::R_lsInternal;
use libr::ENCLOS;
use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::completions::completion_item::completion_item_from_package;
use crate::lsp::completions::completion_item::completion_item_from_symbol;
use crate::lsp::completions::sources::utils::filter_out_dot_prefixes;
use crate::lsp::completions::sources::utils::set_sort_text_by_words_first;
use crate::lsp::completions::types::PromiseStrategy;
use crate::lsp::document_context::DocumentContext;

pub(super) fn completions_from_search_path(
    context: &DocumentContext,
    no_trailing_parens: bool,
) -> Result<Vec<CompletionItem>> {
    log::info!("completions_from_search_path()");

    let mut completions = vec![];

    const R_CONTROL_FLOW_KEYWORDS: &[&str] = &[
        "if", "else", "for", "in", "while", "repeat", "break", "next", "return", "function",
    ];

    unsafe {
        // Iterate through environments starting from the global environment.
        let mut envir = R_GlobalEnv;

        while envir != R_EmptyEnv {
            let is_pkg_env = r_env_is_pkg_env(envir);

            // Get package environment name, if there is one
            let name = if is_pkg_env {
                let name = RObject::from(r_pkg_env_name(envir));
                let name = String::try_from(name)?;
                Some(name)
            } else {
                None
            };

            let name = name.as_deref();

            // If this is a package environment, we will need to force promises to give meaningful completions,
            // particularly with functions because we add a `CompletionItem::command()` that adds trailing `()` onto
            // the completion and triggers parameter completions.
            let promise_strategy = if is_pkg_env {
                PromiseStrategy::Force
            } else {
                PromiseStrategy::Simple
            };

            // List symbols in the environment.
            let symbols = R_lsInternal(envir, 1);

            // Create completion items for each.
            let vector = CharacterVector::new(symbols)?;
            for symbol in vector.iter() {
                // Skip missing values.
                let Some(symbol) = symbol else {
                    continue;
                };

                // Skip control flow keywords.
                let symbol = symbol.as_str();
                if R_CONTROL_FLOW_KEYWORDS.contains(&symbol) {
                    continue;
                }

                // Add the completion item.
                let Some(item) = completion_item_from_symbol(
                    symbol,
                    envir,
                    name,
                    promise_strategy.clone(),
                    no_trailing_parens,
                ) else {
                    log::error!("Completion symbol '{symbol}' was unexpectedly not found.");
                    continue;
                };

                match item {
                    Ok(item) => completions.push(item),
                    Err(error) => log::error!("{:?}", error),
                };
            }

            // Get the next environment.
            envir = ENCLOS(envir);
        }

        // Include installed packages as well.
        // TODO: This can be slow on NFS.
        let packages = RFunction::new("base", ".packages")
            .param("all.available", true)
            .call()?;

        let strings = packages.to::<Vec<String>>()?;
        for string in strings.iter() {
            let item = completion_item_from_package(string, true)?;
            completions.push(item);
        }
    }

    filter_out_dot_prefixes(context, &mut completions);

    // Push search path completions starting with non-word characters to the
    // bottom of the sort list (like those starting with `.`, or `%>%`)
    set_sort_text_by_words_first(&mut completions);

    Ok(completions)
}
