//
// resolve.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::anyhow;
use stdext::*;
use tower_lsp_server::ls_types::CompletionItem;
use tower_lsp_server::ls_types::Documentation;
use tower_lsp_server::ls_types::MarkupContent;
use tower_lsp_server::ls_types::MarkupKind;

use crate::lsp::completions::types::CompletionData;
use crate::lsp::help::RHtmlHelp;

/// The documentation a completion item can be resolved with, rendered by
/// [resolve_completion()] in the session
#[derive(Debug, PartialEq)]
pub(crate) enum ResolveTarget {
    Function {
        name: String,
        package: Option<String>,
    },
    Package {
        name: String,
    },
    Parameter {
        name: String,
        function: String,
    },
}

/// Decodes what resolving `item` needs, or `None` when its kind has no
/// documentation to add. Runs without R, so the handler only hops to the R
/// thread for items that need it.
pub(crate) fn resolve_target(item: &CompletionItem) -> anyhow::Result<Option<ResolveTarget>> {
    let Some(data) = item.data.clone() else {
        return Err(anyhow!(
            "Completion '{label}' has no associated data",
            label = item.label
        ));
    };

    let data: CompletionData = serde_json::from_value(data)
        .map_err(|err| anyhow!("Completion `data` can't be deserialized: {err:?}."))?;

    let target = match data {
        CompletionData::Function { name, package } => ResolveTarget::Function { name, package },
        CompletionData::Package { name } => ResolveTarget::Package { name },
        CompletionData::Parameter { name, function } => ResolveTarget::Parameter { name, function },
        CompletionData::DataVariable { .. } |
        CompletionData::Directory { .. } |
        CompletionData::File { .. } |
        CompletionData::Object { .. } |
        CompletionData::Keyword { .. } |
        CompletionData::RoxygenTag { .. } |
        CompletionData::ScopeVariable { .. } |
        CompletionData::ScopeParameter { .. } |
        CompletionData::Snippet { .. } |
        CompletionData::Unknown => return Ok(None),
    };

    Ok(Some(target))
}

/// Adds the documentation of `target` to `item`. Must run on the R thread.
pub(crate) fn resolve_completion(
    item: &mut CompletionItem,
    target: &ResolveTarget,
) -> anyhow::Result<bool> {
    match target {
        ResolveTarget::Function { name, package } => {
            resolve_function_completion_item(item, name, package.as_deref())
        },
        ResolveTarget::Package { name } => resolve_package_completion_item(item, name),
        ResolveTarget::Parameter { name, function } => {
            resolve_parameter_completion_item(item, name, function)
        },
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

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tower_lsp_server::ls_types::CompletionItem;

    use crate::lsp::completions::resolve::resolve_target;
    use crate::lsp::completions::resolve::ResolveTarget;

    fn item_with_data(data: Option<serde_json::Value>) -> CompletionItem {
        CompletionItem {
            label: "item".to_string(),
            data,
            ..Default::default()
        }
    }

    #[test]
    fn test_resolve_target() {
        let keyword = item_with_data(Some(json!({ "Keyword": { "name": "for" } })));
        assert_eq!(resolve_target(&keyword).unwrap(), None);

        let function = item_with_data(Some(
            json!({ "Function": { "name": "paste", "package": "base" } }),
        ));
        assert_eq!(
            resolve_target(&function).unwrap(),
            Some(ResolveTarget::Function {
                name: String::from("paste"),
                package: Some(String::from("base")),
            })
        );

        assert!(resolve_target(&item_with_data(None)).is_err());
        assert!(resolve_target(&item_with_data(Some(json!({ "Bogus": {} })))).is_err());
    }
}
