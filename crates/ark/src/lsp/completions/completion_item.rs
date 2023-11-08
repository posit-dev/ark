//
// completion_item.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::fs::DirEntry;

use anyhow::bail;
use anyhow::Result;
use harp::r_symbol;
use harp::utils::r_env_binding_is_active;
use harp::utils::r_env_has;
use harp::utils::r_envir_name;
use harp::utils::r_formals;
use harp::utils::r_promise_force_with_rollback;
use harp::utils::r_promise_is_forced;
use harp::utils::r_promise_is_lazy_load_binding;
use harp::utils::r_symbol_quote_invalid;
use harp::utils::r_symbol_valid;
use harp::utils::r_typeof;
use libR_sys::*;
use stdext::*;
use tower_lsp::lsp_types::Command;
use tower_lsp::lsp_types::CompletionItem;
use tower_lsp::lsp_types::CompletionItemKind;
use tower_lsp::lsp_types::Documentation;
use tower_lsp::lsp_types::InsertTextFormat;
use tower_lsp::lsp_types::MarkupContent;
use tower_lsp::lsp_types::MarkupKind;
use tree_sitter::Node;

use crate::lsp::completions::types::CompletionData;
use crate::lsp::completions::types::PromiseStrategy;
use crate::lsp::document_context::DocumentContext;

pub(super) fn completion_item(
    label: impl AsRef<str>,
    data: CompletionData,
) -> Result<CompletionItem> {
    Ok(CompletionItem {
        label: label.as_ref().to_string(),
        data: Some(serde_json::to_value(data)?),
        ..Default::default()
    })
}

pub(super) fn completion_item_from_file(entry: DirEntry) -> Result<CompletionItem> {
    let name = entry.file_name().to_string_lossy().to_string();
    let mut item = completion_item(name, CompletionData::File { path: entry.path() })?;

    item.kind = Some(CompletionItemKind::FILE);
    Ok(item)
}

pub(super) fn completion_item_from_directory(entry: DirEntry) -> Result<CompletionItem> {
    let mut name = entry.file_name().to_string_lossy().to_string();
    name.push_str("/");

    let mut item = completion_item(&name, CompletionData::Directory { path: entry.path() })?;

    item.kind = Some(CompletionItemKind::FOLDER);
    item.command = Some(Command {
        title: "Trigger Suggest".to_string(),
        command: "editor.action.triggerSuggest".to_string(),
        ..Default::default()
    });

    Ok(item)
}

pub(super) fn completion_item_from_direntry(entry: DirEntry) -> Result<CompletionItem> {
    let is_dir = entry
        .file_type()
        .map(|value| value.is_dir())
        .unwrap_or(false);
    if is_dir {
        return completion_item_from_directory(entry);
    } else {
        return completion_item_from_file(entry);
    }
}

pub(super) fn completion_item_from_assignment(
    node: &Node,
    context: &DocumentContext,
) -> Result<CompletionItem> {
    let lhs = node.child_by_field_name("lhs").into_result()?;
    let rhs = node.child_by_field_name("rhs").into_result()?;

    let label = lhs.utf8_text(context.source.as_bytes())?;

    // TODO: Resolve functions that exist in-document here.
    let mut item = completion_item(label, CompletionData::ScopeVariable {
        name: label.to_string(),
    })?;

    let markup = MarkupContent {
        kind: MarkupKind::Markdown,
        value: format!(
            "Defined in this document on line {}.",
            lhs.start_position().row + 1
        ),
    };

    item.detail = Some(label.to_string());
    item.documentation = Some(Documentation::MarkupContent(markup));
    item.kind = Some(CompletionItemKind::VARIABLE);

    if rhs.kind() == "function" {
        if let Some(parameters) = rhs.child_by_field_name("parameters") {
            let parameters = parameters.utf8_text(context.source.as_bytes())?;
            item.detail = Some(join!(label, parameters));
        }

        item.kind = Some(CompletionItemKind::FUNCTION);
        item.insert_text_format = Some(InsertTextFormat::SNIPPET);
        item.insert_text = Some(format!("{}($0)", label));
    }

    return Ok(item);
}

pub(super) unsafe fn completion_item_from_package(
    package: &str,
    append_colons: bool,
) -> Result<CompletionItem> {
    let mut item = completion_item(package.to_string(), CompletionData::Package {
        name: package.to_string(),
    })?;

    item.kind = Some(CompletionItemKind::MODULE);

    if append_colons {
        item.insert_text_format = Some(InsertTextFormat::SNIPPET);
        item.insert_text = Some(format!("{}::", package));
        item.command = Some(Command {
            title: "Trigger Suggest".to_string(),
            command: "editor.action.triggerSuggest".to_string(),
            ..Default::default()
        });
    }

    return Ok(item);
}

pub(super) fn completion_item_from_function<T: AsRef<str>>(
    name: &str,
    package: Option<&str>,
    parameters: &[T],
) -> Result<CompletionItem> {
    let label = format!("{}", name);
    let mut item = completion_item(label, CompletionData::Function {
        name: name.to_string(),
        package: package.map(|s| s.to_string()),
    })?;

    item.kind = Some(CompletionItemKind::FUNCTION);

    let detail = format!("{}({})", name, parameters.joined(", "));
    item.detail = Some(detail);

    item.insert_text_format = Some(InsertTextFormat::SNIPPET);
    item.insert_text = if r_symbol_valid(name) {
        Some(format!("{}($0)", name))
    } else {
        Some(format!("`{}`($0)", name.replace("`", "\\`")))
    };

    // provide parameter completions after completiong function
    item.command = Some(Command {
        title: "Trigger Parameter Hints".to_string(),
        command: "editor.action.triggerParameterHints".to_string(),
        ..Default::default()
    });

    return Ok(item);
}

// TODO
pub(super) unsafe fn completion_item_from_dataset(name: &str) -> Result<CompletionItem> {
    let mut item = completion_item(name.to_string(), CompletionData::Unknown)?;
    item.kind = Some(CompletionItemKind::STRUCT);
    Ok(item)
}

pub(super) unsafe fn completion_item_from_data_variable(
    name: &str,
    owner: &str,
    enquote: bool,
) -> Result<CompletionItem> {
    let mut item = completion_item(name.to_string(), CompletionData::DataVariable {
        name: name.to_string(),
        owner: owner.to_string(),
    })?;

    if enquote {
        item.insert_text = Some(format!("\"{}\"", name));
    }

    item.detail = Some(owner.to_string());
    item.kind = Some(CompletionItemKind::VARIABLE);

    Ok(item)
}

pub(super) unsafe fn completion_item_from_object(
    name: &str,
    object: SEXP,
    envir: SEXP,
    package: Option<&str>,
    promise_strategy: PromiseStrategy,
) -> Result<CompletionItem> {
    if r_typeof(object) == PROMSXP {
        return completion_item_from_promise(name, object, envir, package, promise_strategy);
    }

    // TODO: For some functions (e.g. S4 generics?) the help file might be
    // associated with a separate package. See 'stats4::AIC()' for one example.
    //
    // In other words, when creating a completion item for these functions,
    // we should also figure out where we can receive the help from.
    if Rf_isFunction(object) != 0 {
        let formals = r_formals(object)?;
        let arguments = formals
            .iter()
            .map(|formal| formal.name.as_str())
            .collect::<Vec<_>>();
        return completion_item_from_function(name, package, &arguments);
    }

    let mut item = completion_item(name, CompletionData::Object {
        name: name.to_string(),
    })?;

    item.detail = Some("(Object)".to_string());
    item.kind = Some(CompletionItemKind::STRUCT);

    Ok(item)
}

pub(super) unsafe fn completion_item_from_promise(
    name: &str,
    object: SEXP,
    envir: SEXP,
    package: Option<&str>,
    promise_strategy: PromiseStrategy,
) -> Result<CompletionItem> {
    if r_promise_is_forced(object) {
        // Promise has already been evaluated before.
        // Generate completion item from underlying value.
        let object = PRVALUE(object);
        return completion_item_from_object(name, object, envir, package, promise_strategy);
    }

    if promise_strategy == PromiseStrategy::Force && r_promise_is_lazy_load_binding(object) {
        // TODO: Can we do any better here? Can we avoid evaluation?
        // Namespace completions are the one place we eagerly force unevaluated
        // promises to be able to determine the object type. Particularly
        // important for functions, where we also set a `CompletionItem::command()`
        // to display function signature help after the completion.
        let object = r_promise_force_with_rollback(object)?;
        return completion_item_from_object(name, object, envir, package, promise_strategy);
    }

    // Otherwise we never want to force promises, so we return a fairly
    // generic completion item
    let mut item = completion_item(name, CompletionData::Object {
        name: name.to_string(),
    })?;

    item.detail = Some("Promise".to_string());
    item.kind = Some(CompletionItemKind::STRUCT);

    Ok(item)
}

pub(super) fn completion_item_from_active_binding(name: &str) -> Result<CompletionItem> {
    // We never want to force active bindings, so we return a fairly
    // generic completion item
    let mut item = completion_item(name, CompletionData::Object {
        name: name.to_string(),
    })?;

    item.detail = Some("Active binding".to_string());
    item.kind = Some(CompletionItemKind::STRUCT);

    Ok(item)
}

pub(super) unsafe fn completion_item_from_namespace(
    name: &str,
    namespace: SEXP,
    package: &str,
) -> Result<CompletionItem> {
    // First, look in the namespace itself.
    if let Some(item) =
        completion_item_from_symbol(name, namespace, Some(package), PromiseStrategy::Force)
    {
        return item;
    }

    // Otherwise, try the imports environment.
    let imports = ENCLOS(namespace);
    if let Some(item) =
        completion_item_from_symbol(name, imports, Some(package), PromiseStrategy::Force)
    {
        return item;
    }

    // If still not found, something is wrong.
    bail!(
        "Object '{}' not defined in namespace {:?}",
        name,
        r_envir_name(namespace)?
    )
}

pub(super) unsafe fn completion_item_from_lazydata(
    name: &str,
    env: SEXP,
    package: &str,
) -> Result<CompletionItem> {
    // Important to use `Simple` here, as lazydata bindings are calls to `lazyLoadDBfetch()`
    // but we don't want to force them during completion generation because they often take a
    // long time to load.
    let promise_strategy = PromiseStrategy::Simple;

    match completion_item_from_symbol(name, env, Some(package), promise_strategy) {
        Some(item) => item,
        None => {
            // Should be impossible, but we'll be extra safe
            bail!("Object '{name}' not defined in lazydata environment for namespace {package}")
        },
    }
}

pub(super) unsafe fn completion_item_from_symbol(
    name: &str,
    envir: SEXP,
    package: Option<&str>,
    promise_strategy: PromiseStrategy,
) -> Option<Result<CompletionItem>> {
    let symbol = r_symbol!(name);

    if !r_env_has(envir, symbol) {
        // `r_env_binding_is_active()` will error if the `envir` doesn't contain
        // the symbol in question
        return None;
    }

    if r_env_binding_is_active(envir, symbol) {
        // Active bindings must be checked before `Rf_findVarInFrame()`, as that
        // triggers active bindings
        return Some(completion_item_from_active_binding(name));
    }

    let object = Rf_findVarInFrame(envir, symbol);

    if object == R_UnboundValue {
        log::error!("Symbol '{name}' should have been found.");
        return None;
    }

    Some(completion_item_from_object(
        name,
        object,
        envir,
        package,
        promise_strategy,
    ))
}

// This is used when providing completions for a parameter in a document
// that is considered in-scope at the cursor position.
pub(super) fn completion_item_from_scope_parameter(
    parameter: &str,
    _context: &DocumentContext,
) -> Result<CompletionItem> {
    let mut item = completion_item(parameter, CompletionData::ScopeParameter {
        name: parameter.to_string(),
    })?;

    item.kind = Some(CompletionItemKind::VARIABLE);
    Ok(item)
}

pub(super) unsafe fn completion_item_from_parameter(
    parameter: &str,
    callee: &str,
) -> Result<CompletionItem> {
    let label = r_symbol_quote_invalid(parameter);
    let mut item = completion_item(label, CompletionData::Parameter {
        name: parameter.to_string(),
        function: callee.to_string(),
    })?;

    // TODO: It'd be nice if we could be smarter about how '...' completions are handled,
    // but evidently VSCode doesn't let us set an empty 'insert text' string here.
    // Might be worth fixing upstream.
    item.kind = Some(CompletionItemKind::FIELD);
    item.insert_text_format = Some(InsertTextFormat::SNIPPET);
    item.insert_text = if parameter == "..." {
        Some("...".to_string())
    } else {
        Some(parameter.to_string() + " = ")
    };

    Ok(item)
}
