//
// completion_item.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::fs::DirEntry;

use harp::syntax::is_valid_symbol;
use harp::syntax::sym_quote;
use harp::syntax::sym_quote_invalid;
use stdext::*;
use tower_lsp_server::ls_types::Command;
use tower_lsp_server::ls_types::CompletionItem;
use tower_lsp_server::ls_types::CompletionItemKind;
use tower_lsp_server::ls_types::CompletionItemLabelDetails;
use tower_lsp_server::ls_types::CompletionTextEdit;
use tower_lsp_server::ls_types::Documentation;
use tower_lsp_server::ls_types::InsertTextFormat;
use tower_lsp_server::ls_types::MarkupContent;
use tower_lsp_server::ls_types::MarkupKind;
use tower_lsp_server::ls_types::Position;
use tower_lsp_server::ls_types::Range;
use tower_lsp_server::ls_types::TextEdit;
use tree_sitter::Node;

use crate::lsp::completions::function_context::ArgumentsStatus;
use crate::lsp::completions::function_context::FunctionContext;
use crate::lsp::completions::function_context::FunctionRefUsage;
use crate::lsp::completions::types::CompletionData;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::traits::node::NodeExt;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

pub(super) fn completion_item(
    label: impl AsRef<str>,
    data: CompletionData,
) -> anyhow::Result<CompletionItem> {
    Ok(CompletionItem {
        label: label.as_ref().to_string(),
        data: Some(serde_json::to_value(data)?),
        ..Default::default()
    })
}

pub(super) fn completion_item_from_file(entry: DirEntry) -> anyhow::Result<CompletionItem> {
    let name = entry.file_name().to_string_lossy().to_string();
    let mut item = completion_item(name, CompletionData::File { path: entry.path() })?;

    item.kind = Some(CompletionItemKind::FILE);
    Ok(item)
}

pub(super) fn completion_item_from_directory(entry: DirEntry) -> anyhow::Result<CompletionItem> {
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

pub(super) fn completion_item_from_direntry(entry: DirEntry) -> anyhow::Result<CompletionItem> {
    let is_dir = entry
        .file_type()
        .map(|value| value.is_dir())
        .unwrap_or(false);
    if is_dir {
        completion_item_from_directory(entry)
    } else {
        completion_item_from_file(entry)
    }
}

pub(super) fn completion_item_from_assignment(
    node: &Node,
    context: &DocumentContext,
) -> anyhow::Result<CompletionItem> {
    let lhs = node.child_by_field_name("lhs").into_result()?;
    let rhs = node.child_by_field_name("rhs").into_result()?;

    let label = lhs.node_as_str(context.contents)?.to_string();

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

    // TODO: This ad hoc completion item construction for a function does not
    // benefit from the logic in completion_item_from_function() :(
    if rhs.node_type() == NodeType::FunctionDefinition {
        if let Some(parameters) = rhs.child_by_field_name("parameters") {
            let parameters = parameters.node_as_str(context.contents)?;
            item.detail = Some(join!(label, parameters));
        }

        item.kind = Some(CompletionItemKind::FUNCTION);
        item.insert_text_format = Some(InsertTextFormat::SNIPPET);
        item.insert_text = Some(format!("{}($0)", label));
    }

    Ok(item)
}

pub(super) fn completion_item_from_package(
    package: &str,
    append_colons: bool,
) -> anyhow::Result<CompletionItem> {
    let mut item = completion_item(package, CompletionData::Package {
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

    Ok(item)
}

pub(super) fn completion_item_from_function(
    name: &str,
    package: Option<&str>,
    function_context: &FunctionContext,
) -> anyhow::Result<CompletionItem> {
    let label = name.to_string();
    let mut item = completion_item(label, CompletionData::Function {
        name: name.to_string(),
        package: package.map(|s| s.to_string()),
    })?;

    item.kind = Some(CompletionItemKind::FUNCTION);

    let insert_text = sym_quote_invalid(name);

    let label_details = item_details(package);
    item.label_details = Some(label_details);

    // Are we forming a completion item that's an exact match for an existing
    // function name that is already in the document?
    // This identifies scenarios where we need to edit text, not just insert it.
    let item_is_an_edit = function_context.completes_existing_name(name);

    // These settings are part of the trick we use to make it easy to accept
    // this matching completion item, which will feel like we're just moving
    // the cursor past existing text.
    if item_is_an_edit {
        item.sort_text = Some(format!("0-{name}"));
        item.preselect = Some(true);
    }

    if function_context.arguments_status == ArgumentsStatus::Absent {
        if item_is_an_edit {
            // This is a case like `dplyr::@across` or
            // `debug(dplyr::@across`), i.e. adding the `dplyr::`
            // namespace qualification after-the-fact.
            // We need to consume the existing function name (e.g. `across`) and
            // move the cursor to its end. We don't add parentheses, both
            // because it feels presumptuous and because we don't have a
            // practical way of doing so, in any case. We leave the arguments
            // just as we found them: absent.
            let text_edit = TextEdit {
                range: function_context.range,
                new_text: insert_text,
            };
            item.text_edit = Some(CompletionTextEdit::Edit(text_edit));

            return Ok(item);
        }

        // These are the two most common, plain vanilla function completion
        // scenarios: typing out a known call or reference for the first time.
        match function_context.usage {
            FunctionRefUsage::Call => {
                item.insert_text_format = Some(InsertTextFormat::SNIPPET);
                item.insert_text = Some(format!("{insert_text}($0)"));
                item.command = Some(Command {
                    title: "Trigger Parameter Hints".to_string(),
                    command: "editor.action.triggerParameterHints".to_string(),
                    ..Default::default()
                });
            },
            FunctionRefUsage::Value => {
                item.insert_text_format = Some(InsertTextFormat::PLAIN_TEXT);
                item.insert_text = Some(insert_text);
            },
        }

        return Ok(item);
    }

    // Addresses a sequence that starts like this:
    // some_thing()
    // User realizes they want a different function and backspaces.
    // some_@()
    // User accepts one of the offered completions.
    // some_other_thing(@)
    // User is back on the Happy Path.
    // Also handles the case of editing `some_thing(foo = "bar")`, i.e. where
    // something already exists inside the parentheses.
    // Also handles the case of adding `pkg::` after the fact to an existing
    // call like `pkg::@fcn()` or `pkg::@fcn(foo = "bar)`.
    if function_context.usage == FunctionRefUsage::Call {
        // Tweak the range to cover the opening parenthesis "(" and
        // include the opening parenthesis in the new text.
        // The effect is to move the cursor inside the parentheses.
        let text_edit = TextEdit {
            range: {
                let mut range = function_context.range;
                range.end.character += 1;
                range
            },
            new_text: format!("{}(", insert_text),
        };
        item.text_edit = Some(CompletionTextEdit::Edit(text_edit));
        if function_context.arguments_status == ArgumentsStatus::Empty {
            item.command = Some(Command {
                title: "Trigger Parameter Hints".to_string(),
                command: "editor.action.triggerParameterHints".to_string(),
                ..Default::default()
            });
        }

        return Ok(item);
    }

    // Fallback case which should really never happen, but let's be safe
    log::trace!(
        "completion_item_from_function() - Unexpected case:
         usage={usage:?},
         arguments_status={arguments_status:?},
         name='{name}',
         package={package:?},
         cursor_at_end={cursor_is_at_end}",
        usage = function_context.usage,
        arguments_status = function_context.arguments_status,
        cursor_is_at_end = function_context.cursor_is_at_end
    );

    item.insert_text_format = Some(InsertTextFormat::PLAIN_TEXT);
    item.insert_text = Some(insert_text);

    Ok(item)
}

/// Complete a namespace name whose function kind is unknown. Resolve its
/// documentation by help topic, which also covers non-function objects and
/// help pages without a Usage section, such as concept pages.
pub(super) fn completion_item_from_namespace_object(
    name: &str,
    package: &str,
    function_context: &FunctionContext,
) -> anyhow::Result<CompletionItem> {
    let mut item = completion_item(name, CompletionData::NamespaceObject {
        name: name.to_string(),
        package: package.to_string(),
    })?;

    item.label_details = Some(item_details(Some(package)));
    item.kind = Some(CompletionItemKind::STRUCT);

    let insert_text = sym_quote_invalid(name);

    // In `dplyr::@across`, the name is already in the document. Consume it as
    // `completion_item_from_function()` does, or accepting the item would
    // insert a second copy. That edit adds no parentheses, so it applies
    // whether or not the name is known to be a function.
    if function_context.completes_existing_name(name) {
        item.sort_text = Some(format!("0-{name}"));
        item.preselect = Some(true);
        item.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
            range: function_context.range,
            new_text: insert_text,
        }));
        return Ok(item);
    }

    if !is_valid_symbol(name) {
        item.insert_text = Some(insert_text);
    }

    Ok(item)
}

pub(super) fn item_details(package: Option<&str>) -> CompletionItemLabelDetails {
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
pub(super) fn completion_item_from_dataset(name: &str) -> anyhow::Result<CompletionItem> {
    let mut item = completion_item(name, CompletionData::Unknown)?;
    item.kind = Some(CompletionItemKind::STRUCT);
    Ok(item)
}

#[derive(Copy, Clone)]
pub(super) enum QuoteStyle {
    // Double quote with `"`
    Double,

    // Backtick with `\`` if the symbol isn't syntactic
    BacktickIfNotSyntactic,

    // No quoting. Useful if you know you're already inserting into an existing string.
    None,
}

pub(super) fn completion_item_from_data_variable(
    name: &str,
    owner: &str,
    quote_style: QuoteStyle,
) -> anyhow::Result<CompletionItem> {
    let mut item = completion_item(name, CompletionData::DataVariable {
        name: name.to_string(),
        owner: owner.to_string(),
    })?;

    match quote_style {
        QuoteStyle::Double => item.insert_text = Some(format!("\"{name}\"")),
        QuoteStyle::BacktickIfNotSyntactic => {
            if !is_valid_symbol(name) {
                item.insert_text = Some(sym_quote(name));
            }
        },
        QuoteStyle::None => (),
    };

    item.detail = Some(owner.to_string());
    item.kind = Some(CompletionItemKind::VARIABLE);

    Ok(item)
}

pub(super) fn completion_item_from_variable(name: &str) -> anyhow::Result<CompletionItem> {
    let mut item = completion_item(String::from(name), CompletionData::Object {
        name: String::from(name),
    })?;
    item.kind = Some(CompletionItemKind::VALUE);
    Ok(item)
}

pub(super) fn completion_item_from_active_binding(name: &str) -> anyhow::Result<CompletionItem> {
    // We never want to force active bindings, so we return a fairly
    // generic completion item
    let mut item = completion_item(name, CompletionData::Object {
        name: name.to_string(),
    })?;

    item.detail = Some("Active binding".to_string());
    item.kind = Some(CompletionItemKind::STRUCT);

    if !is_valid_symbol(name) {
        item.insert_text = Some(sym_quote(name));
    }

    Ok(item)
}

// This is used when providing completions for a parameter in a document
// that is considered in-scope at the cursor position.
pub(super) fn completion_item_from_scope_parameter(
    parameter: &str,
    _context: &DocumentContext,
) -> anyhow::Result<CompletionItem> {
    let mut item = completion_item(parameter, CompletionData::ScopeParameter {
        name: parameter.to_string(),
    })?;

    item.kind = Some(CompletionItemKind::VARIABLE);
    Ok(item)
}

/// `position` is the cursor position. The `...` item uses an empty edit there
/// to show its documentation without inserting text.
pub(super) fn completion_item_from_parameter(
    parameter: &str,
    callee: &str,
    position: Position,
) -> anyhow::Result<CompletionItem> {
    if parameter == "..." {
        return completion_item_from_dot_dot_dot(callee, position);
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
    position: Position,
) -> anyhow::Result<CompletionItem> {
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

    let range = Range {
        start: position,
        end: position,
    };
    let text_edit = TextEdit {
        range,
        new_text: "".to_string(),
    };
    item.text_edit = Some(CompletionTextEdit::Edit(text_edit));

    Ok(item)
}
