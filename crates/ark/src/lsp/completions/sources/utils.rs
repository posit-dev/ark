//
// utils.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use tower_lsp::lsp_types::CompletionItem;

pub(super) fn set_sort_text_by_first_appearance(completions: &mut Vec<CompletionItem>) {
    let size = completions.len();

    // Surely there's a better way to figure out what factor of 10 the `size`
    // fits in, but I can't think of it right now
    let mut width = 1;
    let mut value = 10;

    while size >= value {
        value = value * 10;
        width += 1;
    }

    for (i, item) in completions.iter_mut().enumerate() {
        // Start with existing `sort_text` if one exists
        let text = match &item.sort_text {
            Some(sort_text) => sort_text,
            None => &item.label,
        };
        // Append an integer left padded with `0`s
        let prefix = format!("{:0width$}", i, width = width);
        let sort_text = format!("{prefix}-{text}");
        item.sort_text = Some(sort_text);
    }
}
