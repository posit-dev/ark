//
// keyword.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use tower_lsp::lsp_types::CompletionItem;
use tower_lsp::lsp_types::CompletionItemKind;

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
        let mut item = CompletionItem::new_simple(keyword.to_string(), "[keyword]".to_string());
        item.kind = Some(CompletionItemKind::KEYWORD);
        completions.push(item);
    }

    completions
}
