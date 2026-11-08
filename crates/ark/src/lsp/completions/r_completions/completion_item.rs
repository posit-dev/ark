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
use crate::lsp::completions::function_context::ArgumentsStatus;
use crate::lsp::completions::function_context::FunctionContext;
use crate::lsp::completions::function_context::FunctionRefUsage;
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
        // Namespace and package search-path completions force lazy-load
        // bindings to identify functions, whose items insert `()` and trigger
        // signature help. Rollback avoids interrupted-promise warnings if
        // evaluation fails.
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

pub(super) unsafe fn completion_item_from_namespace(
    name: &str,
    namespace: SEXP,
    package: &str,
    function_context: &FunctionContext,
) -> anyhow::Result<CompletionItem> {
    // Re-exported bindings can live in the imports environment rather than
    // the namespace itself, as with dplyr's re-exports of `rlang::.data` and
    // `tidyselect::all_of()`. Try the namespace before its imports environment.
    let error_namespace = match completion_item_from_symbol(
        name,
        namespace,
        Some(package),
        PromiseStrategy::Force,
        function_context,
    ) {
        Ok(item) => return Ok(item),
        Err(error) => error,
    };

    let imports = harp::env_parent(namespace);
    let error_imports = match completion_item_from_symbol(
        name,
        imports,
        Some(package),
        PromiseStrategy::Force,
        function_context,
    ) {
        Ok(item) => return Ok(item),
        Err(error) => error,
    };

    Err(anyhow::anyhow!(
        "Failed to form completion item for '{name}' in namespace '{package}':
        Namespace environment error: {error_namespace}
        Imports environment error: {error_imports}"
    ))
}

pub(super) unsafe fn completion_item_from_lazydata(
    name: &str,
    env: SEXP,
    package: &str,
) -> anyhow::Result<CompletionItem> {
    // Lazydata bindings call `lazyLoadDBfetch()`. Use `PromiseStrategy::Simple`
    // to avoid loading potentially large datasets during completion generation.
    let promise_strategy = PromiseStrategy::Simple;

    // Lazydata is expected to contain datasets, so use a placeholder function context.
    let dummy_function_context = FunctionContext {
        name: String::new(),
        range: Default::default(),
        usage: FunctionRefUsage::Call,
        arguments_status: ArgumentsStatus::Absent,
        cursor_is_at_end: true,
    };

    match completion_item_from_symbol(
        name,
        env,
        Some(package),
        promise_strategy,
        &dummy_function_context,
    ) {
        Ok(item) => Ok(item),
        Err(err) => {
            // Names are listed from this lazydata environment, so a missing
            // binding is unexpected.
            Err(anyhow::anyhow!("Object '{name}' not defined in lazydata environment for namespace {package}: {err}"))
        },
    }
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
            // A missing binding can occur during namespace lookup when `name`
            // is re-exported from the imports environment.
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
