use std::collections::HashSet;

use anyhow::Result;
use regex::Regex;
use stdext::*;
use tower_lsp::lsp_types::CompletionItem;
use tower_lsp::lsp_types::CompletionItemKind;

use crate::lsp::backend::Backend;
use crate::lsp::completions::sources::document::completions_from_document;
use crate::lsp::completions::sources::keyword::completions_from_keywords;
use crate::lsp::completions::sources::search_path::completions_from_search_path;
use crate::lsp::completions::sources::workspace::completions_from_workspace;
use crate::lsp::document_context::DocumentContext;

pub fn completions_from_general_sources(
    backend: &Backend,
    context: &DocumentContext,
) -> Result<Vec<CompletionItem>> {
    let mut completions: Vec<CompletionItem> = vec![];

    completions.append(&mut completions_from_keywords());
    completions.append(&mut completions_from_search_path(context)?);

    if let Some(mut additional_completions) = completions_from_document(context)? {
        completions.append(&mut additional_completions);
    }

    if let Some(mut additional_completions) = completions_from_workspace(backend, context)? {
        completions.append(&mut additional_completions);
    }

    // Remove duplicates
    let mut uniques = HashSet::new();
    completions.retain(|x| uniques.insert(x.label.clone()));

    // sort completions by providing custom 'sort' text to be used when
    // ordering completion results. we use some placeholders at the front
    // to 'bin' different completion types differently; e.g. we place parameter
    // completions at the front, and completions starting with non-word
    // characters at the end (e.g. completions starting with `.`)
    let pattern = Regex::new(r"^\w").unwrap();
    for item in &mut completions {
        case! {

            item.kind == Some(CompletionItemKind::FIELD) => {
                item.sort_text = Some(join!["1", item.label]);
            }

            item.kind == Some(CompletionItemKind::VARIABLE) => {
                item.sort_text = Some(join!["2", item.label]);
            }

            pattern.is_match(&item.label) => {
                item.sort_text = Some(join!["3", item.label]);
            }

            => {
                item.sort_text = Some(join!["4", item.label]);
            }

        }
    }

    Ok(completions)
}
