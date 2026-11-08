//
// search_path.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use harp::env_parent;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::utils::r_env_is_pkg_env;
use harp::utils::r_pkg_env_name;
use harp::vector::CharacterVector;
use harp::vector::Vector;
use harp::RObject;
use libr::R_EmptyEnv;
use libr::R_lsInternal;
use tower_lsp_server::ls_types::CompletionItem;

use crate::console;
use crate::lsp::completions::completion_item::completion_item_from_package;
use crate::lsp::completions::r_completions::completion_item::completion_item_from_symbol;
use crate::lsp::completions::request::SearchPathRequest;
use crate::lsp::completions::sources::utils::remove_dot_prefixes;
use crate::lsp::completions::sources::utils::set_sort_text_by_words_first;
use crate::lsp::completions::types::PromiseStrategy;

/// Symbols from the selected environment and its parents, plus installed
/// packages. Environments are visited in R lookup order. For duplicate
/// completion keys, bindings in nearer environments take precedence.
pub(super) fn r_search_path_completions(
    request: &SearchPathRequest,
) -> anyhow::Result<Vec<CompletionItem>> {
    let SearchPathRequest {
        function_context,
        dot_prefixed,
    } = request;

    let mut completions = vec![];

    const KEYWORD_SOURCE: &[&str] = &[
        "if", "else", "repeat", "while", "function", "for", "in", "next", "break",
    ];

    unsafe {
        let env_obj = console::selected_env();
        let mut env = env_obj.sexp;

        while env != R_EmptyEnv {
            let is_pkg_env = r_env_is_pkg_env(env);

            let name = if is_pkg_env {
                let name = RObject::from(r_pkg_env_name(env));
                let name = String::try_from(name)?;
                Some(name)
            } else {
                None
            };

            let name = name.as_deref();

            // Force package lazy-load bindings to identify functions, whose
            // completion items insert `()` and trigger signature help. Leave
            // promises in user environments unevaluated.
            let promise_strategy = if is_pkg_env {
                PromiseStrategy::Force
            } else {
                PromiseStrategy::Simple
            };

            let symbols = R_lsInternal(env, 1);

            let vector = CharacterVector::new(symbols)?;
            for symbol in vector.iter() {
                let Some(symbol) = symbol else {
                    continue;
                };

                // The static keyword source supplies these items.
                let symbol = symbol.as_str();
                if KEYWORD_SOURCE.contains(&symbol) {
                    continue;
                }

                match completion_item_from_symbol(
                    symbol,
                    env,
                    name,
                    promise_strategy,
                    function_context,
                ) {
                    Ok(item) => completions.push(item),
                    Err(err) => {
                        log::error!("Failed to get completion item for symbol '{symbol}': {err}");
                        continue;
                    },
                };
            }

            env = env_parent(env);
        }

        // TODO: Listing all installed packages can be slow on NFS.
        let packages = RFunction::new("base", ".packages")
            .param("all.available", true)
            .call()?;

        let strings = packages.to::<Vec<String>>()?;
        for string in strings.iter() {
            let item = completion_item_from_package(string, true)?;
            completions.push(item);
        }
    }

    if !dot_prefixed {
        remove_dot_prefixes(&mut completions);
    }

    // Prefer ordinary names over dot-prefixed names and operators such as `%>%`.
    set_sort_text_by_words_first(&mut completions);

    Ok(completions)
}
