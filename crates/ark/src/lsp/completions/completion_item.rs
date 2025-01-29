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
use harp::utils::is_symbol_valid;
use harp::utils::r_env_binding_is_active;
use harp::utils::r_envir_name;
use harp::utils::r_promise_force_with_rollback;
use harp::utils::r_promise_is_forced;
use harp::utils::r_promise_is_lazy_load_binding;
use harp::utils::r_typeof;
use harp::utils::sym_quote;
use harp::utils::sym_quote_invalid;
use libr::R_UnboundValue;
use libr::Rf_findVarInFrame;
use libr::Rf_isFunction;
use libr::ENCLOS;
use libr::PROMSXP;
use libr::PRVALUE;
use libr::SEXP;
use stdext::*;
use tower_lsp::lsp_types::Command;
use tower_lsp::lsp_types::CompletionItem;
use tower_lsp::lsp_types::CompletionItemKind;
use tower_lsp::lsp_types::CompletionItemLabelDetails;
use tower_lsp::lsp_types::CompletionTextEdit;
use tower_lsp::lsp_types::Documentation;
use tower_lsp::lsp_types::InsertTextFormat;
use tower_lsp::lsp_types::MarkupContent;
use tower_lsp::lsp_types::MarkupKind;
use tower_lsp::lsp_types::Range;
use tower_lsp::lsp_types::TextEdit;
use tree_sitter::Node;

use crate::lsp::completions::types::CompletionData;
use crate::lsp::completions::types::PromiseStrategy;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::encoding::convert_point_to_position;
use crate::lsp::traits::rope::RopeExt;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

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
    name.push('/');

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

    let label = context.document.contents.node_slice(&lhs)?.to_string();

    // TODO: Resolve functions that exist in-document here.
    let mut item = completion_item(label.clone(), CompletionData::ScopeVariable {
        name: label.clone(),
    })?;

    let markup = MarkupContent {
        kind: MarkupKind::Markdown,
        value: format!(
            "Defined in this document on line {}.",
            lhs.start_position().row + 1
        ),
    };

    item.detail = Some(label.clone());
    item.documentation = Some(Documentation::MarkupContent(markup));
    item.kind = Some(CompletionItemKind::VARIABLE);

    if rhs.node_type() == NodeType::FunctionDefinition {
        if let Some(parameters) = rhs.child_by_field_name("parameters") {
            let parameters = context
                .document
                .contents
                .node_slice(&parameters)?
                .to_string();
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
    item.label_details = Some(CompletionItemLabelDetails {
        detail: Some(String::from("::")),
        description: None,
    });

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

pub(super) fn completion_item_from_function(
    name: &str,
    package: Option<&str>,
    no_trailing_parens: bool,
) -> Result<CompletionItem> {
    let label = format!("{}", name);
    let mut item = completion_item(label, CompletionData::Function {
        name: name.to_string(),
        package: package.map(|s| s.to_string()),
    })?;

    item.kind = Some(CompletionItemKind::FUNCTION);

    let label_details = item_details(package);
    item.label_details = Some(label_details);

    let insert_text = sym_quote_invalid(name);

    if no_trailing_parens {
        item.insert_text_format = Some(InsertTextFormat::PLAIN_TEXT);
        item.insert_text = Some(insert_text);
    } else {
        item.insert_text_format = Some(InsertTextFormat::SNIPPET);
        item.insert_text = Some(format!("{insert_text}($0)"));

        // provide parameter completions after completing function
        item.command = Some(Command {
            title: "Trigger Parameter Hints".to_string(),
            command: "editor.action.triggerParameterHints".to_string(),
            ..Default::default()
        });
    }

    Ok(item)
}

fn item_details(package: Option<&str>) -> CompletionItemLabelDetails {
    let description = package.map(|p| {
        // Environments from the search path often have a "package:" prefix.
        // Remove it from display. This creates some rare ambiguities but
        // improves the display generally.
        let p = p.strip_prefix("package:").unwrap_or(p);
        format!("{{{p}}}")
    });

    CompletionItemLabelDetails {
        detail: None,
        description,
    }
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
    } else if !is_symbol_valid(name) {
        item.insert_text = Some(sym_quote(name));
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
    no_parens: bool,
) -> Result<CompletionItem> {
    if r_typeof(object) == PROMSXP {
        return completion_item_from_promise(
            name,
            object,
            envir,
            package,
            promise_strategy,
            no_parens,
        );
    }

    // TODO: For some functions (e.g. S4 generics?) the help file might be
    // associated with a separate package. See 'stats4::AIC()' for one example.
    //
    // In other words, when creating a completion item for these functions,
    // we should also figure out where we can receive the help from.
    if Rf_isFunction(object) != 0 {
        return completion_item_from_function(name, package, no_parens);
    }

    let mut item = completion_item(name, CompletionData::Object {
        name: name.to_string(),
    })?;

    item.label_details = Some(item_details(package));
    item.kind = Some(CompletionItemKind::STRUCT);

    if !is_symbol_valid(name) {
        item.insert_text = Some(sym_quote(name));
    }

    Ok(item)
}

pub(super) fn completion_item_from_variable(name: &str) -> anyhow::Result<CompletionItem> {
    let mut item = completion_item(String::from(name), CompletionData::Object {
        name: String::from(name),
    })?;
    item.kind = Some(CompletionItemKind::VALUE);
    Ok(item)
}

pub(super) unsafe fn completion_item_from_promise(
    name: &str,
    object: SEXP,
    envir: SEXP,
    package: Option<&str>,
    promise_strategy: PromiseStrategy,
    no_parens: bool,
) -> Result<CompletionItem> {
    if r_promise_is_forced(object) {
        // Promise has already been evaluated before.
        // Generate completion item from underlying value.
        let object = PRVALUE(object);
        return completion_item_from_object(
            name,
            object,
            envir,
            package,
            promise_strategy,
            no_parens,
        );
    }

    if promise_strategy == PromiseStrategy::Force && r_promise_is_lazy_load_binding(object) {
        // TODO: Can we do any better here? Can we avoid evaluation?
        // Namespace completions are the one place we eagerly force unevaluated
        // promises to be able to determine the object type. Particularly
        // important for functions, where we also set a `CompletionItem::command()`
        // to display function signature help after the completion.
        let object = r_promise_force_with_rollback(object)?;
        return completion_item_from_object(
            name,
            object.sexp,
            envir,
            package,
            promise_strategy,
            no_parens,
        );
    }

    // Otherwise we never want to force promises, so we return a fairly
    // generic completion item
    let mut item = completion_item(name, CompletionData::Object {
        name: name.to_string(),
    })?;

    item.detail = Some("Promise".to_string());
    item.kind = Some(CompletionItemKind::STRUCT);

    if !is_symbol_valid(name) {
        item.insert_text = Some(sym_quote(name));
    }

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

    if !is_symbol_valid(name) {
        item.insert_text = Some(sym_quote(name));
    }

    Ok(item)
}

pub(super) unsafe fn completion_item_from_namespace(
    name: &str,
    namespace: SEXP,
    package: &str,
    no_trailing_parens: bool,
) -> Result<CompletionItem> {
    // First, look in the namespace itself.
    if let Some(item) = completion_item_from_symbol(
        name,
        namespace,
        Some(package),
        PromiseStrategy::Force,
        no_trailing_parens,
    ) {
        return item;
    }

    // Otherwise, try the imports environment.
    let imports = ENCLOS(namespace);
    if let Some(item) =
        completion_item_from_symbol(name, imports, Some(package), PromiseStrategy::Force, false)
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

    match completion_item_from_symbol(name, env, Some(package), promise_strategy, false) {
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
    no_parens: bool,
) -> Option<Result<CompletionItem>> {
    let symbol = r_symbol!(name);

    match r_env_binding_is_active(envir, symbol) {
        Ok(false) => {
            // Continue with standard environment completion item creation
            ()
        },
        Ok(true) => {
            // We can't even extract out the object for active bindings so they
            // are handled extremely specially.
            return Some(completion_item_from_active_binding(name));
        },
        Err(err) => {
            log::error!("Can't determine if binding is active: {err:?}");
            return None;
        },
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
        no_parens,
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

pub(super) fn completion_item_from_parameter(
    parameter: &str,
    callee: &str,
    context: &DocumentContext,
) -> Result<CompletionItem> {
    if parameter == "..." {
        return completion_item_from_dot_dot_dot(callee, context);
    }

    // `data` captured using original `parameter`, before quoting
    let data = CompletionData::Parameter {
        name: parameter.to_string(),
        function: callee.to_string(),
    };

    let parameter = sym_quote_invalid(parameter);

    // We want to display to the user the name with the `=`
    let label = parameter.clone() + " = ";

    let mut item = completion_item(label.as_str(), data)?;

    item.kind = Some(CompletionItemKind::FIELD);

    // We want to insert the name with the `=` too
    item.insert_text = Some(label);
    item.insert_text_format = Some(InsertTextFormat::SNIPPET);

    // But we filter and sort on the label without the `=`
    item.filter_text = Some(parameter.clone());
    item.sort_text = Some(parameter.clone());

    Ok(item)
}

fn completion_item_from_dot_dot_dot(
    callee: &str,
    context: &DocumentContext,
) -> Result<CompletionItem> {
    // Special behavior for `...` arguments, where we want to show them
    // in quick suggestions (to show help docs for them), but not actually
    // insert any text for them if the user selects them. Can't use an
    // `insert_text` of `""` because Positron treats it like `None`.
    let label = "...";

    let mut item = completion_item(label, CompletionData::Parameter {
        name: label.to_string(),
        function: callee.to_string(),
    })?;

    item.kind = Some(CompletionItemKind::FIELD);

    let position = convert_point_to_position(&context.document.contents, context.point);

    let range = Range {
        start: position,
        end: position,
    };
    let textedit = TextEdit {
        range,
        new_text: "".to_string(),
    };
    let textedit = CompletionTextEdit::Edit(textedit);
    item.text_edit = Some(textedit);

    Ok(item)
}
