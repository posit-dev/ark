//
// comment.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::path::Path;

use anyhow::Result;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use regex::Regex;
use stdext::unwrap;
use tower_lsp::lsp_types::CompletionItem;
use tower_lsp::lsp_types::Documentation;
use tower_lsp::lsp_types::InsertTextFormat;
use tower_lsp::lsp_types::MarkupContent;
use tower_lsp::lsp_types::MarkupKind;
use yaml_rust::YamlLoader;

use crate::lsp::completions::completion_item::completion_item;
use crate::lsp::completions::types::CompletionData;
use crate::lsp::document_context::DocumentContext;

pub fn completions_from_comment(context: &DocumentContext) -> Result<Option<Vec<CompletionItem>>> {
    log::info!("completions_from_comment()");

    let node = context.node;

    if node.kind() != "comment" {
        return Ok(None);
    }

    let pattern = Regex::new(r"^.*\s")?;

    let contents = node.utf8_text(context.source.as_bytes())?;
    let token = pattern.replace(contents, "");

    let mut completions: Vec<CompletionItem> = vec![];

    if !token.starts_with('@') {
        // We are done, there are no completions, but we are in a comment so
        // no one else should get a chance to register anything
        return Ok(Some(completions));
    }

    // TODO: cache these?
    // TODO: use an indexer to build the tag list?
    let tags = unsafe {
        RFunction::new("base", "system.file")
            .param("package", "roxygen2")
            .add("roxygen2-tags.yml")
            .call()?
            .to::<String>()?
    };

    if tags.is_empty() {
        return Ok(Some(completions));
    }

    let tags = Path::new(&tags);
    if !tags.exists() {
        return Ok(Some(completions));
    }

    let contents = std::fs::read_to_string(tags).unwrap();
    let docs = YamlLoader::load_from_str(contents.as_str()).unwrap();
    let doc = &docs[0];

    let items = doc.as_vec().unwrap();
    for entry in items.iter() {
        let name = unwrap!(entry["name"].as_str(), None => {
            continue;
        });

        let label = name.to_string();
        let mut item = completion_item(label.clone(), CompletionData::RoxygenTag {
            tag: label.clone(),
        })?;

        // TODO: What is the appropriate icon for us to use here?
        let template = entry["template"].as_str();
        if let Some(template) = template {
            item.insert_text_format = Some(InsertTextFormat::SNIPPET);
            item.insert_text = Some(format!("{}{}", name, template));
        } else {
            item.insert_text = Some(format!("@{}", label.as_str()));
        }

        item.detail = Some(format!("roxygen @{} (R)", name));
        if let Some(description) = entry["description"].as_str() {
            let markup = MarkupContent {
                kind: MarkupKind::Markdown,
                value: description.to_string(),
            };
            item.documentation = Some(Documentation::MarkupContent(markup));
        }

        completions.push(item);
    }

    Ok(Some(completions))
}
