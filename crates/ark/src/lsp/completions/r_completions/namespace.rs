//
// namespace.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::r_symbol;
use libr::R_UnboundValue;
use libr::R_lsInternal;
use libr::Rboolean_TRUE;
use libr::Rf_findVarInFrame;
use libr::SEXP;
use tower_lsp_server::ls_types::CompletionItem;

use crate::lsp::completions::function_context::FunctionContext;
use crate::lsp::completions::r_completions::completion_item::completion_item_from_lazydata;
use crate::lsp::completions::r_completions::completion_item::completion_item_from_namespace;
use crate::lsp::completions::sources::utils::set_sort_text_by_words_first;

/// Symbols of `package`'s namespace after `pkg::` (exports and lazydata) or
/// `pkg:::` (all symbols). Loads the namespace if needed.
pub(super) fn r_namespace_completions(
    package: &str,
    exports_only: bool,
    function_context: &FunctionContext,
) -> anyhow::Result<Vec<CompletionItem>> {
    let mut completions: Vec<CompletionItem> = vec![];

    // Get the package namespace
    let Ok(namespace) = RFunction::new("base", "getNamespace").add(package).call() else {
        // An unavailable namespace yields no items. Recognition of `::` or
        // `:::` suppresses other completion sources.
        return Ok(completions);
    };

    let symbols = if package == "base" {
        list_namespace_symbols(*namespace)
    } else if exports_only {
        list_namespace_exports(*namespace)
    } else {
        list_namespace_symbols(*namespace)
    };

    let strings = symbols.to::<Vec<String>>()?;

    for string in strings.iter() {
        let item = unsafe {
            completion_item_from_namespace(string, *namespace, package, function_context)
        };
        match item {
            Ok(item) => completions.push(item),
            Err(error) => log::error!("{error:?}"),
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

    Ok(completions)
}

fn completions_from_namespace_lazydata(
    namespace: SEXP,
    package: &str,
) -> anyhow::Result<Option<Vec<CompletionItem>>> {
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

        if names.is_empty() {
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
    unsafe { RObject::new(R_lsInternal(namespace, 1)) }
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

        RObject::new(R_lsInternal(exports, 1))
    }
}

#[cfg(test)]
mod tests {
    use crate::lsp::completions::tests::utils::find_completion_by_label;
    use crate::lsp::completions::tests::utils::get_completions_at_cursor;
    use crate::r_task;

    #[test]
    fn test_completions_after_colons() {
        r_task(|| {
            // Just colons, no RHS text yet
            let completions = get_completions_at_cursor("utils::@").unwrap();

            let item = find_completion_by_label(&completions, "adist");
            assert!(item.is_some());

            // Should not find internal function
            let item = find_completion_by_label(&completions, "as.bibentry.bibentry");
            assert!(item.is_none());

            // Internal functions with `:::`
            let completions = get_completions_at_cursor("utils:::@").unwrap();
            let item = find_completion_by_label(&completions, "as.bibentry.bibentry");
            assert!(item.is_some());

            // With RHS text, which is ignored when generating completions.
            // Filtering applied on frontend side.
            let completions = get_completions_at_cursor("utils::bl@ah").unwrap();
            let item = find_completion_by_label(&completions, "adist");
            assert!(item.is_some());
        });
    }

    #[test]
    fn test_empty_set_of_completions_when_using_unknown_package() {
        // https://github.com/posit-dev/ark/issues/833
        r_task(|| {
            let completions = get_completions_at_cursor("foo::bar@").unwrap();
            assert!(completions.is_empty());
        });
    }
}
