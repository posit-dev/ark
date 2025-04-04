//
// resolve.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::bail;
use stdext::*;
use tower_lsp::lsp_types::CompletionItem;
use tower_lsp::lsp_types::Documentation;
use tower_lsp::lsp_types::MarkupContent;
use tower_lsp::lsp_types::MarkupKind;

use crate::lsp::completions::types::CompletionData;
use crate::lsp::help::RHtmlHelp;

pub fn resolve_completion(item: &mut CompletionItem) -> anyhow::Result<bool> {
    let Some(data) = item.data.clone() else {
        bail!("Completion '{}' has no associated data", item.label);
    };

    let data: CompletionData = unwrap!(serde_json::from_value(data), Err(err) => {
        bail!("Completion `data` can't be deserialized: {err:?}.");
    });

    match data {
        CompletionData::DataVariable { name: _, owner: _ } => Ok(false),
        CompletionData::Directory { path: _ } => Ok(false),
        CompletionData::File { path: _ } => Ok(false),
        CompletionData::Function { name, package } => {
            resolve_function_completion_item(item, name.as_str(), package.as_deref())
        },
        CompletionData::Package { name } => resolve_package_completion_item(item, name.as_str()),
        CompletionData::Parameter { name, function } => {
            resolve_parameter_completion_item(item, name.as_str(), function.as_str())
        },
        CompletionData::Object { name: _ } => Ok(false),
        CompletionData::Keyword { name: _ } => Ok(false),
        CompletionData::RoxygenTag { tag: _ } => Ok(false),
        CompletionData::ScopeVariable { name: _ } => Ok(false),
        CompletionData::ScopeParameter { name: _ } => Ok(false),
        CompletionData::Snippet { text: _ } => Ok(false),
        CompletionData::Unknown => Ok(false),
    }
}

fn resolve_package_completion_item(
    item: &mut CompletionItem,
    package: &str,
) -> anyhow::Result<bool> {
    let topic = join!(package, "-package");
    let help = unwrap!(RHtmlHelp::from_topic(topic.as_str(), Some(package))?, None => {
        return Ok(false);
    });

    let markup = help.markdown()?;
    let markup = MarkupContent {
        kind: MarkupKind::Markdown,
        value: markup.to_string(),
    };

    item.detail = None;
    item.documentation = Some(Documentation::MarkupContent(markup));

    Ok(true)
}

fn resolve_function_completion_item(
    item: &mut CompletionItem,
    name: &str,
    package: Option<&str>,
) -> anyhow::Result<bool> {
    let help = unwrap!(RHtmlHelp::from_function(name, package)?, None => {
        return Ok(false);
    });

    let markup = help.markdown()?;

    let markup = MarkupContent {
        kind: MarkupKind::Markdown,
        value: markup,
    };

    item.documentation = Some(Documentation::MarkupContent(markup));

    Ok(true)
}

// TODO: Include package as well here?
fn resolve_parameter_completion_item(
    item: &mut CompletionItem,
    name: &str,
    function: &str,
) -> anyhow::Result<bool> {
    // Get help for this function.
    let help = unwrap!(RHtmlHelp::from_function(function, None)?, None => {
        return Ok(false);
    });

    // Extract the relevant parameter help.
    let markup = unwrap!(help.parameter(name)?, None => {
        return Ok(false);
    });

    // Build the actual markup content.
    // We found it; amend the documentation.
    item.detail = Some(format!("{}()", function));
    item.documentation = Some(Documentation::MarkupContent(markup));
    Ok(true)
}
