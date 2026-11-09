//
// completion_item.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use harp::r_symbol;
use harp::syntax::is_valid_symbol;
use harp::syntax::sym_quote;
use harp::utils::r_env_binding_is_active;
use harp::utils::r_promise_force_with_rollback;
use harp::utils::r_promise_is_forced;
use harp::utils::r_promise_is_lazy_load_binding;
use harp::utils::r_typeof;
use libr::R_UnboundValue;
use libr::Rf_findVarInFrame;
use libr::Rf_isFunction;
use libr::PROMSXP;
use libr::PRVALUE;
use libr::SEXP;
use tower_lsp_server::ls_types::CompletionItem;
use tower_lsp_server::ls_types::CompletionItemKind;

use crate::lsp::completions::completion_item::completion_item;
use crate::lsp::completions::completion_item::completion_item_from_active_binding;
use crate::lsp::completions::completion_item::completion_item_from_function;
use crate::lsp::completions::completion_item::item_details;
use crate::lsp::completions::function_context::FunctionContext;
use crate::lsp::completions::types::CompletionData;
use crate::lsp::completions::types::PromiseStrategy;

pub(super) unsafe fn completion_item_from_object(
    name: &str,
    object: SEXP,
    envir: SEXP,
    package: Option<&str>,
    promise_strategy: PromiseStrategy,
    function_context: &FunctionContext,
) -> anyhow::Result<CompletionItem> {
    if r_typeof(object) == PROMSXP {
        return completion_item_from_promise(
            name,
            object,
            envir,
            package,
            promise_strategy,
            function_context,
        );
    }

    // TODO: Resolve the help package separately from the binding's package.
    // For example, `stats4::AIC()` has help in another package.
    if Rf_isFunction(object) != 0 {
        return completion_item_from_function(name, package, function_context);
    }

    let mut item = completion_item(name, CompletionData::Object {
        name: name.to_string(),
    })?;

    item.label_details = Some(item_details(package));
    item.kind = Some(CompletionItemKind::STRUCT);

    if !is_valid_symbol(name) {
        item.insert_text = Some(sym_quote(name));
    }

    Ok(item)
}

pub(super) unsafe fn completion_item_from_promise(
    name: &str,
    object: SEXP,
    envir: SEXP,
    package: Option<&str>,
    promise_strategy: PromiseStrategy,
    function_context: &FunctionContext,
) -> anyhow::Result<CompletionItem> {
    if r_promise_is_forced(object) {
        let object = PRVALUE(object);
        return completion_item_from_object(
            name,
            object,
            envir,
            package,
            promise_strategy,
            function_context,
        );
    }

    if promise_strategy == PromiseStrategy::Force && r_promise_is_lazy_load_binding(object) {
        // Search-path completions force lazy-load bindings to identify
        // functions, whose items insert `()` and trigger signature help.
        // Rollback avoids interrupted-promise warnings if evaluation fails.
        // TODO: Determine lazy-load binding types without evaluation.
        let object = r_promise_force_with_rollback(object)?;
        return completion_item_from_object(
            name,
            object.sexp,
            envir,
            package,
            promise_strategy,
            function_context,
        );
    }

    // Leave other promises unevaluated to avoid running user code for completions.
    let mut item = completion_item(name, CompletionData::Object {
        name: name.to_string(),
    })?;

    item.detail = Some("Promise".to_string());
    item.kind = Some(CompletionItemKind::STRUCT);

    if !is_valid_symbol(name) {
        item.insert_text = Some(sym_quote(name));
    }

    Ok(item)
}

pub(super) unsafe fn completion_item_from_symbol(
    name: &str,
    envir: SEXP,
    package: Option<&str>,
    promise_strategy: PromiseStrategy,
    function_context: &FunctionContext,
) -> anyhow::Result<CompletionItem> {
    let symbol = r_symbol!(name);

    match r_env_binding_is_active(envir, symbol) {
        Ok(false) => {},
        Ok(true) => {
            // Reading an active binding executes its function. Return a
            // generic item without retrieving the value.
            return completion_item_from_active_binding(name);
        },
        Err(err) => {
            return Err(anyhow::anyhow!(
                "Failed to check if binding is active: {err}"
            ));
        },
    }

    let object = Rf_findVarInFrame(envir, symbol);

    if object == R_UnboundValue {
        return Err(anyhow::anyhow!(
            "Symbol '{name}' should have been found but wasn't"
        ));
    }

    completion_item_from_object(
        name,
        object,
        envir,
        package,
        promise_strategy,
        function_context,
    )
}
