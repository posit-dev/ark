//
// keyword.rs
//
// Copyright (C) 2023-2025 Posit Software, PBC. All rights reserved.
//
//

use stdext::unwrap;
use tower_lsp::lsp_types::CompletionItem;
use tower_lsp::lsp_types::CompletionItemKind;
use tower_lsp::lsp_types::Documentation;
use tower_lsp::lsp_types::InsertTextFormat;
use tower_lsp::lsp_types::MarkupContent;
use tower_lsp::lsp_types::MarkupKind;

use crate::lsp::completions::completion_context::CompletionContext;
use crate::lsp::completions::completion_item::completion_item;
use crate::lsp::completions::sources::CompletionSource;
use crate::lsp::completions::types::CompletionData;

pub(super) struct KeywordSource;

impl CompletionSource for KeywordSource {
    fn name(&self) -> &'static str {
        "keyword"
    }

    fn provide_completions(
        &self,
        _completion_context: &CompletionContext,
    ) -> anyhow::Result<Option<Vec<CompletionItem>>> {
        completions_from_keywords()
    }
}

pub fn completions_from_keywords() -> anyhow::Result<Option<Vec<CompletionItem>>> {
    let mut completions = vec![];

    add_bare_keywords(&mut completions);
    add_keyword_snippets(&mut completions);

    Ok(Some(completions))
}

const BARE_KEYWORDS: &[&str] = &[
    "NULL",
    "NA",
    "TRUE",
    "FALSE",
    "Inf",
    "NaN",
    "NA_integer_",
    "NA_real_",
    "NA_character_",
    "NA_complex_",
    "in",
    "next",
    "break",
    "repeat",
    "function",
];

// Snippet data is a tuple of:
// - keyword: The reserved word
// - label: The label for the completion item
// - snippet: The snippet to be inserted
// - detail: The detail displayed in the completion UI

// The only case where `keyword != label` is `fun` for `function`.
// But in the name of preserving original behaviour, this is my opening
// move.
const KEYWORD_SNIPPETS: &[(&str, &str, &str, &str)] = &[
    // (keyword, label, snippet, detail)
    (
        "for",
        "for",
        "for (${1:variable} in ${2:vector}) {\n\t${0}\n}",
        "Define a loop",
    ),
    (
        "if",
        "if",
        "if (${1:condition}) {\n\t${0}\n}",
        "Conditional expression",
    ),
    (
        "while",
        "while",
        "while (${1:condition}) {\n\t${0}\n}",
        "Define a loop",
    ),
    (
        "else",
        "else",
        "else {\n\t${0}\n}",
        "Conditional expression",
    ),
    (
        "function",
        "fun",
        "${1:name} <- function(${2:variables}) {\n\t${0}\n}",
        "Function skeleton",
    ),
];

fn add_bare_keywords(completions: &mut Vec<CompletionItem>) {
    for keyword in BARE_KEYWORDS {
        let item = completion_item(keyword.to_string(), CompletionData::Keyword {
            name: keyword.to_string(),
        });

        let mut item = unwrap!(item, Err(err) => {
            log::error!("Failed to construct completion item for keyword '{keyword}' due to {err:?}.");
            continue;
        });

        item.detail = Some("[keyword]".to_string());
        item.kind = Some(CompletionItemKind::KEYWORD);

        completions.push(item);
    }
}

fn add_keyword_snippets(completions: &mut Vec<CompletionItem>) {
    for (keyword, label, snippet, label_details_description) in KEYWORD_SNIPPETS {
    for (keyword, label, snippet, detail) in KEYWORD_SNIPPETS {
        let item = completion_item(label.to_string(), CompletionData::Snippet {
            text: snippet.to_string(),
        });

        let mut item = match item {
            Ok(item) => item,
            Err(err) => {
                log::trace!("Failed to construct completion item for reserved keyword '{keyword}' due to {err:?}");
                continue;
            },
        };

        // Markup shows up in the quick suggestion documentation window,
        // so you can see what the snippet expands to
        let markup = vec!["```r", snippet, "```"].join("\n");
        let markup = MarkupContent {
            kind: MarkupKind::Markdown,
            value: markup,
        };

        item.detail = Some(detail.to_string());
        item.documentation = Some(Documentation::MarkupContent(markup));
        item.kind = Some(CompletionItemKind::SNIPPET);
        item.insert_text = Some(snippet.to_string());
        item.insert_text_format = Some(InsertTextFormat::SNIPPET);

        completions.push(item);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_presence_bare_keywords() {
        let completions = super::completions_from_keywords().unwrap().unwrap();

        for keyword in super::BARE_KEYWORDS {
            let item = completions.iter().find(|item| item.label == *keyword);
            assert!(
                item.is_some(),
                "Expected keyword '{}' not found in completions",
                keyword
            );
            let item = item.unwrap();
            assert_eq!(item.detail, Some("[keyword]".to_string()));
            assert_eq!(
                item.kind,
                Some(tower_lsp::lsp_types::CompletionItemKind::KEYWORD)
            );
        }
    }

    #[test]
    fn test_presence_keyword_snippets() {
        let completions = super::completions_from_keywords().unwrap().unwrap();

        let snippet_labels: Vec<&str> = super::KEYWORD_SNIPPETS
            .iter()
            .map(|(_, label, _, _)| *label)
            .collect();

        for label in snippet_labels {
            let item = completions.iter().find(|item| item.label == label);
            assert!(
                item.is_some(),
                "Expected snippet '{}' not found in completions",
                label
            );
            let item = item.unwrap();
            assert_eq!(
                item.kind,
                Some(tower_lsp::lsp_types::CompletionItemKind::SNIPPET)
            );
        }
    }
}
