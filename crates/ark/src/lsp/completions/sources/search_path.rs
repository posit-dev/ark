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
use harp::utils::r_envir_name;
use harp::vector::CharacterVector;
use harp::vector::Vector;
use libR_sys::R_EmptyEnv;
use libR_sys::R_GlobalEnv;
use libR_sys::R_lsInternal;
use libR_sys::ENCLOS;
use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::completions::completion_item::completion_item_from_package;
use crate::lsp::completions::completion_item::completion_item_from_symbol;
use crate::lsp::completions::types::PromiseStrategy;
use crate::lsp::document_context::DocumentContext;

pub(super) fn completions_from_search_path(
    context: &DocumentContext,
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
            // Get environment name
            let name = r_envir_name(envir)?;

            // If this is a package environment, we will need to force promises to give meaningful completions,
            // particularly with functions because we add a `CompletionItem::command()` that adds trailing `()` onto
            // the completion and triggers parameter completions.
            let promise_strategy = if r_env_is_pkg_env(envir) {
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
                    Some(name.as_str()),
                    promise_strategy.clone(),
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

    // Remove completions that start with `.` unless the user explicitly requested them
    let user_requested_dot = context
        .node
        .utf8_text(context.source.as_bytes())
        .and_then(|x| Ok(x.starts_with(".")))
        .unwrap_or(false);

    if !user_requested_dot {
        completions.retain(|x| !x.label.starts_with("."));
    }

    Ok(completions)
}
