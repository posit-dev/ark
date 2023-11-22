//
// namespace.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::Result;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::r_symbol;
use libR_sys::R_UnboundValue;
use libR_sys::R_lsInternal;
use libR_sys::Rboolean_TRUE;
use libR_sys::Rf_findVarInFrame;
use libR_sys::SEXP;
use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::completions::completion_item::completion_item_from_lazydata;
use crate::lsp::completions::completion_item::completion_item_from_namespace;
use crate::lsp::completions::sources::utils::set_sort_text_by_words_first;
use crate::lsp::document_context::DocumentContext;

// Handle the case with 'package::prefix', where the user has now
// started typing the prefix of the symbol they would like completions for.
pub fn completions_from_namespace(
    context: &DocumentContext,
) -> Result<Option<Vec<CompletionItem>>> {
    log::info!("completions_from_namespace()");

    let mut node = context.node;

    let mut has_namespace_completions = false;
    let mut exports_only = false;

    loop {
        // Must check for named nodes, otherwise literal `::` operators
        // (with no children) come through
        if node.is_named() && matches!(node.kind(), "::" | ":::") {
            exports_only = node.kind() == "::";
            has_namespace_completions = true;
            break;
        }

        // If we reach a brace list, bail.
        if node.kind() == "{" {
            break;
        }

        // Update the node.
        node = match node.parent() {
            Some(node) => node,
            None => break,
        };
    }

    if !has_namespace_completions {
        return Ok(None);
    }

    let mut completions: Vec<CompletionItem> = vec![];

    let Some(node) = node.child(0) else {
        return Ok(Some(completions));
    };

    let package = node.utf8_text(context.source.as_bytes())?;

    // Get the package namespace.
    let namespace = unsafe { RFunction::new("base", "getNamespace").add(package).call()? };

    let symbols = if package == "base" {
        list_namespace_symbols(*namespace)
    } else if exports_only {
        list_namespace_exports(*namespace)
    } else {
        list_namespace_symbols(*namespace)
    };

    let strings = unsafe { symbols.to::<Vec<String>>()? };

    for string in strings.iter() {
        let item = unsafe { completion_item_from_namespace(string, *namespace, package) };
        match item {
            Ok(item) => completions.push(item),
            Err(error) => log::error!("{:?}", error),
        }
    }

    if exports_only {
        // `pkg:::object` doesn't return lazy objects, so we don't want
        // to show lazydata completions if we are inside `:::`
        let lazydata = completions_from_namespace_lazydata(*namespace, package)?;
        if let Some(mut lazydata) = lazydata {
            completions.append(&mut lazydata);
        }
    }

    set_sort_text_by_words_first(&mut completions);

    Ok(Some(completions))
}

fn completions_from_namespace_lazydata(
    namespace: SEXP,
    package: &str,
) -> Result<Option<Vec<CompletionItem>>> {
    log::info!("completions_from_namespace_lazydata()");

    unsafe {
        let ns = Rf_findVarInFrame(namespace, r_symbol!(".__NAMESPACE__."));
        if ns == R_UnboundValue {
            return Ok(None);
        }

        let env = Rf_findVarInFrame(ns, r_symbol!("lazydata"));
        if env == R_UnboundValue {
            return Ok(None);
        }

        let names = RObject::to::<Vec<String>>(RObject::from(R_lsInternal(env, Rboolean_TRUE)))?;

        if names.len() == 0 {
            return Ok(None);
        }

        let mut completions: Vec<CompletionItem> = vec![];

        for name in names.iter() {
            match completion_item_from_lazydata(name, env, package) {
                Ok(item) => completions.push(item),
                Err(error) => log::error!("{:?}", error),
            }
        }

        Ok(Some(completions))
    }
}

fn list_namespace_symbols(namespace: SEXP) -> RObject {
    return unsafe { RObject::new(R_lsInternal(namespace, 1)) };
}

fn list_namespace_exports(namespace: SEXP) -> RObject {
    unsafe {
        let ns = Rf_findVarInFrame(namespace, r_symbol!(".__NAMESPACE__."));
        if ns == R_UnboundValue {
            return RObject::null();
        }

        let exports = Rf_findVarInFrame(ns, r_symbol!("exports"));
        if exports == R_UnboundValue {
            return RObject::null();
        }

        return RObject::new(R_lsInternal(exports, 1));
    }
}
