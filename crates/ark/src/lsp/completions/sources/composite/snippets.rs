//
// snippets.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::collections::HashMap;
use std::sync::LazyLock;

use rust_embed::RustEmbed;
use serde::Deserialize;
use tower_lsp::lsp_types::CompletionItem;
use tower_lsp::lsp_types::CompletionItemKind;
use tower_lsp::lsp_types::Documentation;
use tower_lsp::lsp_types::InsertTextFormat;
use tower_lsp::lsp_types::MarkupContent;
use tower_lsp::lsp_types::MarkupKind;

use crate::lsp::completions::completion_item::completion_item;
use crate::lsp::completions::types::CompletionData;

#[derive(RustEmbed)]
#[folder = "resources/snippets/"]
struct Asset;

#[derive(Deserialize)]
struct Snippet {
    prefix: String,
    body: SnippetBody,
    description: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SnippetBody {
    Scalar(String),
    Vector(Vec<String>),
}

pub(super) fn completions_from_snippets() -> Vec<CompletionItem> {
    log::info!("completions_from_snippets()");

    // Return clone of cached snippet completion items
    let completions = get_completions_from_snippets().clone();

    completions
}

fn get_completions_from_snippets() -> &'static Vec<CompletionItem> {
    static SNIPPETS: LazyLock<Vec<CompletionItem>> =
        LazyLock::new(|| init_completions_from_snippets());
    &SNIPPETS
}

fn init_completions_from_snippets() -> Vec<CompletionItem> {
    // Load snippets JSON from embedded file
    let file = Asset::get("r.code-snippets").unwrap();
    let snippets: HashMap<String, Snippet> = serde_json::from_slice(&file.data).unwrap();

    let mut completions = vec![];

    for snippet in snippets.values() {
        let label = snippet.prefix.clone();
        let details = snippet.description.clone();

        let body = match &snippet.body {
            SnippetBody::Scalar(body) => body.clone(),
            SnippetBody::Vector(body) => body.join("\n"),
        };

        // Markup shows up in the quick suggestion documentation window,
        // so you can see what the snippet expands to
        let markup = vec!["```r", body.as_str(), "```"].join("\n");
        let markup = MarkupContent {
            kind: MarkupKind::Markdown,
            value: markup,
        };

        let mut item =
            completion_item(label, CompletionData::Snippet { text: body.clone() }).unwrap();

        item.detail = Some(details);
        item.documentation = Some(Documentation::MarkupContent(markup));
        item.kind = Some(CompletionItemKind::SNIPPET);

        item.insert_text = Some(body);
        item.insert_text_format = Some(InsertTextFormat::SNIPPET);

        completions.push(item);
    }

    completions
}

#[cfg(test)]
mod tests {
    use crate::lsp::completions::sources::composite::snippets::completions_from_snippets;

    #[test]
    fn test_snippets() {
        let snippets = completions_from_snippets();

        // Hash map isn't stable with regards to ordering
        let item = snippets.iter().find(|item| item.label == "lib").unwrap();
        assert_eq!(item.detail, Some("Attach an R package".to_string()));
        assert_eq!(item.insert_text, Some("library(${1:package})".to_string()));

        // Multiline body
        let item = snippets.iter().find(|item| item.label == "if").unwrap();
        assert_eq!(
            item.insert_text,
            Some("if (${1:condition}) {\n\t${0}\n}".to_string())
        );
    }
}
