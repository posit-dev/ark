//
// search_path.rs
//
// Copyright (C) 2023-2025 Posit Software, PBC. All rights reserved.
//
//

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

use crate::lsp::completions::completion_context::CompletionContext;
use crate::lsp::completions::completion_item::completion_item_from_package;
use crate::lsp::completions::completion_item::completion_item_from_symbol;
use crate::lsp::completions::sources::utils::filter_out_dot_prefixes;
use crate::lsp::completions::sources::utils::set_sort_text_by_words_first;
use crate::lsp::completions::sources::CompletionSource;
use crate::lsp::completions::types::PromiseStrategy;

pub(super) struct SearchPathSource;

impl CompletionSource for SearchPathSource {
    fn name(&self) -> &'static str {
        "search_path"
    }

    fn provide_completions(
        &self,
        completion_context: &CompletionContext,
    ) -> anyhow::Result<Option<Vec<CompletionItem>>> {
        completions_from_search_path(completion_context)
    }
}

fn completions_from_search_path(
    context: &CompletionContext,
) -> anyhow::Result<Option<Vec<CompletionItem>>> {
    let mut completions = vec![];

    const KEYWORD_SOURCE: &[&str] = &[
        "if", "else", "repeat", "while", "function", "for", "in", "next", "break",
    ];

    unsafe {
        // Iterate through environments starting from the global environment.
        let mut env = R_GlobalEnv;

        // If we're waiting for input in `read_console()` with a debugger
        // prompt, start from current environment
        #[cfg(not(test))] // Unit tests do not have an `RMain`
        {
            use crate::interface::RMain;
            if let Some(debug_env) = &RMain::get().debug_env() {
                // Mem-Safety: Object protected by `RMain` for the duration of the `r_task()`
                env = debug_env.sexp;
            }
        }

        while env != R_EmptyEnv {
            let is_pkg_env = r_env_is_pkg_env(env);

            // Get package environment name, if there is one
            let name = if is_pkg_env {
                let name = RObject::from(r_pkg_env_name(env));
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
            let symbols = R_lsInternal(env, 1);

            // Create completion items for each.
            let vector = CharacterVector::new(symbols)?;
            for symbol in vector.iter() {
                // Skip missing values.
                let Some(symbol) = symbol else {
                    continue;
                };

                // Skip anything that is covered by the keyword source.
                let symbol = symbol.as_str();
                if KEYWORD_SOURCE.contains(&symbol) {
                    continue;
                }

                // Add the completion item.
                match completion_item_from_symbol(
                    symbol,
                    env,
                    name,
                    promise_strategy,
                    context.function_context(),
                ) {
                    Ok(item) => completions.push(item),
                    Err(err) => {
                        // Log the error but continue processing other symbols
                        log::error!("Failed to get completion item for symbol '{symbol}': {err}");
                        continue;
                    },
                };
            }

            // Get the next environment.
            env = ENCLOS(env);
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

    filter_out_dot_prefixes(context.document_context, &mut completions);

    // Push search path completions starting with non-word characters to the
    // bottom of the sort list (like those starting with `.`, or `%>%`)
    set_sort_text_by_words_first(&mut completions);

    Ok(Some(completions))
}
