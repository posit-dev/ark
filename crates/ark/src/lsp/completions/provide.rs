//
// provide.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::Result;
use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::completions::completion_utils::check_for_function_value;
use crate::lsp::completions::completion_utils::check_for_help;
use crate::lsp::completions::completion_utils::gather_completion_context;
use crate::lsp::completions::sources::completions_from_composite_sources;
use crate::lsp::completions::sources::completions_from_unique_sources;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::state::WorldState;

// Entry point for completions.
// Must be within an `r_task()`.
pub(crate) fn provide_completions(
    context: &DocumentContext,
    state: &WorldState,
) -> Result<Vec<CompletionItem>> {
    log::info!("provide_completions()");

    let node_context = gather_completion_context(context);
    let no_trailing_parens =
        check_for_function_value(context, &node_context) || check_for_help(&node_context);

    if let Some(completions) = completions_from_unique_sources(context, no_trailing_parens)? {
        return Ok(completions);
    };

    // At this point we aren't in a "unique" completion case, so just return a
    // set of reasonable completions based on loaded packages, the open
    // document, the current workspace, and any call related arguments
    completions_from_composite_sources(context, state, no_trailing_parens)
}

#[cfg(test)]
mod tests {
    use tower_lsp::lsp_types::InsertTextFormat;
    use tree_sitter::Point;

    use super::*;
    use crate::lsp::document_context::DocumentContext;
    use crate::lsp::documents::Document;
    use crate::lsp::state::WorldState;
    use crate::r_task;

    fn point_from_cursor(text: &str) -> (String, Point) {
        let cursor_pos = text.find('@').unwrap();
        let text = text.replace('@', "");
        (text, Point::new(0, cursor_pos))
    }

    fn find_completion(completions: &[CompletionItem], label: &str) -> CompletionItem {
        completions
            .iter()
            .find(|item| item.label == label)
            .unwrap()
            .clone()
    }

    #[test]
    fn test_completions_dont_add_parentheses_inside_special_functions_naked() {
        r_task(|| {
            let (text, point) = point_from_cursor("debug(enc@)");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let completions = provide_completions(&context, &WorldState::default()).unwrap();
            let completion = find_completion(&completions, "enc2native");

            // (1) correct string (no trailing parens)
            // (2) plain text, not a snippet with a placeholder for the cursor
            // (3) no extra command to trigger parameter hints
            assert_eq!(completion.insert_text.unwrap(), String::from("enc2native"));
            assert_eq!(
                completion.insert_text_format.unwrap(),
                InsertTextFormat::PLAIN_TEXT
            );
            assert!(completion.command.is_none());
        })
    }

    #[test]
    fn test_completions_dont_add_parentheses_inside_special_functions_double_colon() {
        r_task(|| {
            let (text, point) = point_from_cursor("debug(base::ab@)");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let completions = provide_completions(&context, &WorldState::default()).unwrap();
            let completion = find_completion(&completions, "abs");

            assert_eq!(completion.insert_text.unwrap(), String::from("abs"));
            assert_eq!(
                completion.insert_text_format.unwrap(),
                InsertTextFormat::PLAIN_TEXT
            );
            assert!(completion.command.is_none());
        })
    }

    #[test]
    fn test_completions_dont_add_parentheses_inside_special_functions_triple_colon() {
        r_task(|| {
            let (text, point) = point_from_cursor("debug(utils:::.get@)");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let completions = provide_completions(&context, &WorldState::default()).unwrap();
            let completion = find_completion(&completions, ".getHelpFile");

            assert_eq!(
                completion.insert_text.unwrap(),
                String::from(".getHelpFile")
            );
            assert_eq!(
                completion.insert_text_format.unwrap(),
                InsertTextFormat::PLAIN_TEXT
            );
            assert!(completion.command.is_none());
        })
    }

    #[test]
    fn test_completions_dont_add_parentheses_for_help_operator_naked() {
        r_task(|| {
            let (text, point) = point_from_cursor("?enc@");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let completions = provide_completions(&context, &WorldState::default()).unwrap();
            let completion = find_completion(&completions, "enc2native");

            assert_eq!(completion.insert_text.unwrap(), String::from("enc2native"));
            assert_eq!(
                completion.insert_text_format.unwrap(),
                InsertTextFormat::PLAIN_TEXT
            );
            assert!(completion.command.is_none());
        })
    }

    #[test]
    fn test_completions_dont_add_parentheses_for_help_operator_double_colon() {
        r_task(|| {
            let (text, point) = point_from_cursor("?base::enc@");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let completions = provide_completions(&context, &WorldState::default()).unwrap();
            let completion = find_completion(&completions, "enc2native");

            assert_eq!(completion.insert_text.unwrap(), String::from("enc2native"));
            assert_eq!(
                completion.insert_text_format.unwrap(),
                InsertTextFormat::PLAIN_TEXT
            );
            assert!(completion.command.is_none());
        })
    }
}
