//
// keyword.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use stdext::unwrap;
use tower_lsp::lsp_types::CompletionItem;
use tower_lsp::lsp_types::CompletionItemKind;

use crate::lsp::completions::completion_item::completion_item;
use crate::lsp::completions::types::CompletionData;

pub(super) fn completions_from_keywords() -> Vec<CompletionItem> {
    log::info!("completions_from_keywords()");

    let mut completions = vec![];

    // provide keyword completion results
    // NOTE: Some R keywords have definitions provided in the R
    // base namespace, so we don't need to provide duplicate
    // definitions for these here.
    let keywords = vec![
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
        "else",
        "next",
        "break",
    ];

    for keyword in keywords {
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

    completions
}
