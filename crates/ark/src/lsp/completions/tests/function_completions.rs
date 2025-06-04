//
// completions/tests/function_completions.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
//

#[cfg(test)]
mod function_call_tests {
    use tower_lsp::lsp_types::CompletionItemKind;
    use tower_lsp::lsp_types::InsertTextFormat;

    use crate::lsp::completions::tests::utils::assert_has_parameter_hints;
    use crate::lsp::completions::tests::utils::find_completion_by_label;
    use crate::lsp::completions::tests::utils::get_completions_at_cursor;
    use crate::r_task;

    #[test]
    fn test_basic_call() {
        r_task(|| {
            let completions = get_completions_at_cursor("a@").unwrap();
            let item = find_completion_by_label(&completions, "abbreviate");

            assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
            assert_eq!(item.insert_text, Some("abbreviate($0)".to_string()));
            assert_eq!(item.insert_text_format, Some(InsertTextFormat::SNIPPET));
            assert_has_parameter_hints(item);
        });
    }

    #[test]
    fn test_call_with_namespace() {
        r_task(|| {
            let completions = get_completions_at_cursor("utils::a@").unwrap();
            let item = find_completion_by_label(&completions, "adist");

            assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
            assert_eq!(item.insert_text, Some("adist($0)".to_string()));
            assert_eq!(item.insert_text_format, Some(InsertTextFormat::SNIPPET));
            assert_has_parameter_hints(item);
        });
    }

    #[test]
    fn test_call_inside_call() {
        r_task(|| {
            let completions = get_completions_at_cursor("rev(tou@").unwrap();
            let item = find_completion_by_label(&completions, "toupper");

            assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
            assert_eq!(item.insert_text, Some("toupper($0)".to_string()));
            assert_eq!(item.insert_text_format, Some(InsertTextFormat::SNIPPET));
            assert_has_parameter_hints(item);
        });
    }
}

#[cfg(test)]
mod function_reference_tests {
    use tower_lsp::lsp_types::CompletionItemKind;
    use tower_lsp::lsp_types::InsertTextFormat;

    use crate::lsp::completions::tests::utils::assert_no_command;
    use crate::lsp::completions::tests::utils::assert_text_edit;
    use crate::lsp::completions::tests::utils::find_completion_by_label;
    use crate::lsp::completions::tests::utils::get_completions_at_cursor;
    use crate::r_task;

    #[test]
    fn test_basic_reference() {
        r_task(|| {
            let completions = get_completions_at_cursor("debug(a@)").unwrap();
            let item = find_completion_by_label(&completions, "any");

            assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
            assert_eq!(item.insert_text, Some("any".to_string()));
            assert_eq!(item.insert_text_format, Some(InsertTextFormat::PLAIN_TEXT));
            assert_no_command(item);
        });
    }

    #[test]
    fn test_unary_help() {
        r_task(|| {
            let completions = get_completions_at_cursor("?a@").unwrap();
            let item = find_completion_by_label(&completions, "any");

            assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
            assert_eq!(item.insert_text, Some("any".to_string()));
            assert_eq!(item.insert_text_format, Some(InsertTextFormat::PLAIN_TEXT));
            assert_no_command(item);
        });
    }

    #[test]
    fn test_reference_with_namespace() {
        r_task(|| {
            let completions = get_completions_at_cursor("debug(utils::a@)").unwrap();
            let item = find_completion_by_label(&completions, "adist");

            assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
            assert_eq!(item.insert_text, Some("adist".to_string()));
            assert_eq!(item.insert_text_format, Some(InsertTextFormat::PLAIN_TEXT));
            assert_no_command(item);
        });
    }

    #[test]
    fn test_reference_text_match() {
        r_task(|| {
            let completions = get_completions_at_cursor("debug(any@)").unwrap();
            let item = find_completion_by_label(&completions, "any");

            assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
            assert_text_edit(&item, "any");
        });
    }
}

/// Tests where namespace is added retroactively
#[cfg(test)]
mod namespace_post_hoc_tests {
    use tower_lsp::lsp_types::CompletionItemKind;
    use tower_lsp::lsp_types::InsertTextFormat;

    use crate::lsp::completions::tests::utils::assert_has_parameter_hints;
    use crate::lsp::completions::tests::utils::assert_text_edit;
    use crate::lsp::completions::tests::utils::find_completion_by_label;
    use crate::lsp::completions::tests::utils::get_completions_at_cursor;
    use crate::r_task;

    #[test]
    fn test_function_call_with_post_hoc_namespace() {
        r_task(|| {
            // Somewhat artifical in that you rarely to feel the need to add
            // `utils::` here, given that utils is generally available. But I
            // want to use a base package and this still tests the mechanics.
            let completions = get_completions_at_cursor("utils::@adist").unwrap();
            let item = find_completion_by_label(&completions, "adist");

            assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
            assert_eq!(item.insert_text, Some("adist($0)".to_string()));
            assert_eq!(item.insert_text_format, Some(InsertTextFormat::SNIPPET));
            assert_has_parameter_hints(item);
        });
    }

    #[test]
    fn test_function_call_with_post_hoc_namespace_empty_arguments() {
        r_task(|| {
            let completions = get_completions_at_cursor("utils::@adist()").unwrap();
            let item = find_completion_by_label(&completions, "adist");

            assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
            assert_text_edit(item, "adist(");
            assert_has_parameter_hints(item);
        });
    }

    #[test]
    fn test_function_call_with_post_hoc_namespace_nonempty_arguments() {
        r_task(|| {
            let completions = get_completions_at_cursor("utils::@adist()").unwrap();
            let item = find_completion_by_label(&completions, "adist");

            assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
            assert_text_edit(item, "adist(");
            assert_has_parameter_hints(item);
        });
    }

    #[test]
    fn test_reference_with_post_hoc_namespace() {
        r_task(|| {
            let completions = get_completions_at_cursor("debug(utils::@adist)").unwrap();
            let item = find_completion_by_label(&completions, "adist");

            assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
            assert_text_edit(item, "adist");
        });
    }
}

#[cfg(test)]
mod empty_args_tests {
    use tower_lsp::lsp_types::CompletionItemKind;

    use crate::lsp::completions::tests::utils::assert_has_parameter_hints;
    use crate::lsp::completions::tests::utils::assert_text_edit;
    use crate::lsp::completions::tests::utils::find_completion_by_label;
    use crate::lsp::completions::tests::utils::get_completions_at_cursor;
    use crate::r_task;

    #[test]
    fn test_empty_parentheses_no_function() {
        r_task(|| {
            let completions = get_completions_at_cursor("@()").unwrap();
            assert!(completions.is_empty())
        });
    }

    #[test]
    fn test_function_name_with_empty_parentheses() {
        r_task(|| {
            let completions = get_completions_at_cursor("a@()").unwrap();
            let item = find_completion_by_label(&completions, "abbreviate");

            assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
            assert_text_edit(item, "abbreviate(");
            assert_has_parameter_hints(item);
        });
    }

    #[test]
    fn test_namespace_function_with_empty_parentheses() {
        r_task(|| {
            let completions = get_completions_at_cursor("utils::@adist()").unwrap();
            let item = find_completion_by_label(&completions, "adist");

            assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
            assert_text_edit(item, "adist(");
        });
    }
}

#[cfg(test)]
mod nonempty_args_tests {
    use tower_lsp::lsp_types::CompletionItemKind;

    use crate::lsp::completions::tests::utils::assert_no_command;
    use crate::lsp::completions::tests::utils::assert_text_edit;
    use crate::lsp::completions::tests::utils::find_completion_by_label;
    use crate::lsp::completions::tests::utils::get_completions_at_cursor;
    use crate::r_task;

    #[test]
    fn test_function_name_with_nonempty_parentheses() {
        r_task(|| {
            let completions = get_completions_at_cursor("a@(\"hello\", width=10)").unwrap();
            let item = find_completion_by_label(&completions, "abbreviate");

            assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
            assert_text_edit(item, "abbreviate");

            assert_no_command(item);
        });
    }

    #[test]
    fn test_namespace_function_with_nonempty_parentheses() {
        r_task(|| {
            let completions = get_completions_at_cursor("utils::a@(\"hi\", \"bye\")").unwrap();
            let item = find_completion_by_label(&completions, "adist");

            assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
            assert_text_edit(item, "adist");

            assert_no_command(item);
        });
    }
}
