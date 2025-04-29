//
// keyword.rs
//
// Copyright (C) 2023-2025 Posit Software, PBC. All rights reserved.
//
//

use stdext::unwrap;
use tower_lsp::lsp_types::CompletionItem;
use tower_lsp::lsp_types::CompletionItemKind;
use tower_lsp::lsp_types::CompletionItemLabelDetails;
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
    "TRUE",
    "FALSE",
    "NULL",
    "Inf",
    "NaN",
    "NA",
    "NA_integer_",
    "NA_real_",
    "NA_complex_",
    "NA_character_",
    "if",
    "else",
    "repeat",
    "while",
    "function",
    "for",
    "in",
    "next",
    "break",
];

struct KeywordSnippet {
    keyword: &'static str,
    label: &'static str,
    snippet: &'static str,
    label_details_description: &'static str,
}

const KEYWORD_SNIPPETS: &[KeywordSnippet] = &[
    KeywordSnippet {
        keyword: "if",
        label: "if",
        snippet: "if (${1:condition}) {\n\t${0}\n}",
        label_details_description: "An if statement",
    },
    KeywordSnippet {
        keyword: "else",
        label: "else",
        snippet: "else {\n\t${0}\n}",
        label_details_description: "An else statement",
    },
    KeywordSnippet {
        keyword: "repeat",
        label: "repeat",
        snippet: "repeat {\n\t${0}\n}",
        label_details_description: "A repeat loop",
    },
    KeywordSnippet {
        keyword: "while",
        label: "while",
        snippet: "while (${1:condition}) {\n\t${0}\n}",
        label_details_description: "A while loop",
    },
    KeywordSnippet {
        keyword: "function",
        label: "fun",
        snippet: "${1:name} <- function(${2:variables}) {\n\t${0}\n}",
        label_details_description: "Define a function",
    },
    KeywordSnippet {
        keyword: "for",
        label: "for",
        snippet: "for (${1:variable} in ${2:vector}) {\n\t${0}\n}",
        label_details_description: "A for loop",
    },
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

        item.kind = Some(CompletionItemKind::KEYWORD);
        item.label_details = Some(CompletionItemLabelDetails {
            detail: None,
            description: Some("[keyword]".to_string()),
        });

        completions.push(item);
    }
}

fn add_keyword_snippets(completions: &mut Vec<CompletionItem>) {
    for KeywordSnippet {
        keyword,
        label,
        snippet,
        label_details_description,
    } in KEYWORD_SNIPPETS
    {
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

        item.documentation = Some(Documentation::MarkupContent(markup));
        item.kind = Some(CompletionItemKind::SNIPPET);
        item.insert_text = Some(snippet.to_string());
        item.insert_text_format = Some(InsertTextFormat::SNIPPET);
        item.label_details = Some(CompletionItemLabelDetails {
            detail: None,
            description: Some(label_details_description.to_string()),
        });

        completions.push(item);
    }
}

#[cfg(test)]
mod tests {
    use tower_lsp::lsp_types::CompletionItemLabelDetails;

    #[test]
    fn test_presence_bare_keywords() {
        let completions = super::completions_from_keywords().unwrap().unwrap();
        let keyword_completions: Vec<_> = completions
            .iter()
            .filter(|item| item.kind == Some(tower_lsp::lsp_types::CompletionItemKind::KEYWORD))
            .collect();

        for keyword in super::BARE_KEYWORDS {
            let item = keyword_completions
                .iter()
                .find(|item| item.label == *keyword);
            assert!(
                item.is_some(),
                "Expected keyword '{keyword}' not found in completions"
            );
            let item = item.unwrap();
            assert_eq!(
                item.label_details,
                Some(CompletionItemLabelDetails {
                    detail: None,
                    description: Some("[keyword]".to_string()),
                })
            );
        }
    }

    #[test]
    fn test_presence_keyword_snippets() {
        let completions = super::completions_from_keywords().unwrap().unwrap();
        let snippet_completions: Vec<_> = completions
            .iter()
            .filter(|item| item.kind == Some(tower_lsp::lsp_types::CompletionItemKind::SNIPPET))
            .collect();

        let snippet_labels: Vec<&str> = super::KEYWORD_SNIPPETS
            .iter()
            .map(|snippet| snippet.label)
            .collect();

        for label in snippet_labels {
            let item = snippet_completions.iter().find(|item| item.label == label);
            assert!(
                item.is_some(),
                "Expected snippet '{label}' with SNIPPET kind not found in completions"
            );
        }
    }
}
