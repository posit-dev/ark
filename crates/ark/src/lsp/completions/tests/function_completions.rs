//
// function_completions.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
//

#[cfg(test)]
mod function_call_tests {
    use tower_lsp_server::ls_types::CompletionItemKind;
    use tower_lsp_server::ls_types::InsertTextFormat;

    use crate::lsp::completions::tests::utils::assert_has_parameter_hints;
    use crate::lsp::completions::tests::utils::find_completion_by_label;
    use crate::lsp::completions::tests::utils::get_completions_at_cursor;
    use crate::lsp::completions::tests::utils::static_items_with_test_package;
    use crate::r_task;

    #[test]
    fn test_basic_call() {
        r_task(|| {
            let completions = get_completions_at_cursor("a@").unwrap();
            let item = find_completion_by_label(&completions, "abbreviate");

            assert!(item.is_some());
            let item = item.unwrap();

            assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
            assert_eq!(item.insert_text, Some("abbreviate($0)".to_string()));
            assert_eq!(item.insert_text_format, Some(InsertTextFormat::SNIPPET));
            assert_has_parameter_hints(item);
        });
    }

    #[test]
    fn test_call_with_namespace() {
        let completions = static_items_with_test_package("pkg::a@");
        let item = find_completion_by_label(&completions, "adist");

        assert!(item.is_some());
        let item = item.unwrap();

        assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
        assert_eq!(item.insert_text, Some("adist($0)".to_string()));
        assert_eq!(item.insert_text_format, Some(InsertTextFormat::SNIPPET));
        assert_has_parameter_hints(item);
    }

    #[test]
    fn test_call_inside_call() {
        r_task(|| {
            let completions = get_completions_at_cursor("rev(tou@").unwrap();
            let item = find_completion_by_label(&completions, "toupper");

            assert!(item.is_some());
            let item = item.unwrap();

            assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
            assert_eq!(item.insert_text, Some("toupper($0)".to_string()));
            assert_eq!(item.insert_text_format, Some(InsertTextFormat::SNIPPET));
            assert_has_parameter_hints(item);
        });
    }

    #[test]
    fn test_call_exact_match() {
        r_task(|| {
            let completions = get_completions_at_cursor("dir@").unwrap();
            let item = find_completion_by_label(&completions, "dir");

            assert!(item.is_some());
            let item = item.unwrap();

            assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
            assert_eq!(item.insert_text, Some("dir($0)".to_string()));
            assert_eq!(item.insert_text_format, Some(InsertTextFormat::SNIPPET));
            assert_has_parameter_hints(item);
        });
    }
}

#[cfg(test)]
mod function_reference_tests {
    use tower_lsp_server::ls_types::CompletionItemKind;
    use tower_lsp_server::ls_types::InsertTextFormat;

    use crate::lsp::completions::tests::utils::assert_no_command;
    use crate::lsp::completions::tests::utils::find_completion_by_label;
    use crate::lsp::completions::tests::utils::get_completions_at_cursor;
    use crate::lsp::completions::tests::utils::static_items_with_test_package;
    use crate::r_task;

    #[test]
    fn test_basic_reference() {
        r_task(|| {
            let completions = get_completions_at_cursor("debug(a@)").unwrap();
            let item = find_completion_by_label(&completions, "any");

            assert!(item.is_some());
            let item = item.unwrap();

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

            assert!(item.is_some());
            let item = item.unwrap();

            assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
            assert_eq!(item.insert_text, Some("any".to_string()));
            assert_eq!(item.insert_text_format, Some(InsertTextFormat::PLAIN_TEXT));
            assert_no_command(item);
        });
    }

    #[test]
    fn test_reference_with_namespace() {
        let completions = static_items_with_test_package("debug(pkg::a@)");
        let item = find_completion_by_label(&completions, "adist");

        assert!(item.is_some());
        let item = item.unwrap();

        assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
        assert_eq!(item.insert_text, Some("adist".to_string()));
        assert_eq!(item.insert_text_format, Some(InsertTextFormat::PLAIN_TEXT));
        assert_no_command(item);
    }

    #[test]
    fn test_reference_text_match() {
        r_task(|| {
            let completions = get_completions_at_cursor("debug(any@)").unwrap();
            let item = find_completion_by_label(&completions, "any");

            assert!(item.is_some());
            let item = item.unwrap();

            assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
            assert_eq!(item.insert_text, Some("any".to_string()));
            assert_eq!(item.insert_text_format, Some(InsertTextFormat::PLAIN_TEXT));
            assert_no_command(item);
        });
    }
}

/// Tests where namespace is added retroactively
#[cfg(test)]
mod namespace_post_hoc_tests {
    use tower_lsp_server::ls_types::CompletionItemKind;
    use tower_lsp_server::ls_types::InsertTextFormat;

    use crate::lsp::completions::tests::utils::assert_has_parameter_hints;
    use crate::lsp::completions::tests::utils::assert_sort_text_has_priority_prefix;
    use crate::lsp::completions::tests::utils::assert_text_edit;
    use crate::lsp::completions::tests::utils::find_completion_by_label;
    use crate::lsp::completions::tests::utils::static_items_with_state;
    use crate::lsp::completions::tests::utils::static_items_with_test_package;
    use crate::lsp::completions::tests::utils::write_library_package;

    #[test]
    fn test_function_call_with_post_hoc_namespace() {
        let completions = static_items_with_test_package("pkg::@adist");

        let item = find_completion_by_label(&completions, "adist");
        assert!(item.is_some());
        let item = item.unwrap();

        assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
        assert_text_edit(item, "adist");
        assert_sort_text_has_priority_prefix(item);
        assert_eq!(item.preselect, Some(true));

        let item = find_completion_by_label(&completions, "aspell");
        assert!(item.is_some());
        let item = item.unwrap();

        assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
        assert_eq!(item.insert_text, Some("aspell($0)".to_string()));
        assert_eq!(item.insert_text_format, Some(InsertTextFormat::SNIPPET));
        assert_has_parameter_hints(item);
    }

    #[test]
    fn test_function_call_with_post_hoc_namespace_empty_arguments() {
        let completions = static_items_with_test_package("pkg::@adist()");
        let item = find_completion_by_label(&completions, "adist");

        assert!(item.is_some());
        let item = item.unwrap();

        assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
        assert_text_edit(item, "adist(");
        assert_has_parameter_hints(item);
    }

    #[test]
    fn test_function_call_with_post_hoc_namespace_nonempty_arguments() {
        let completions = static_items_with_test_package("pkg::@adist()");
        let item = find_completion_by_label(&completions, "adist");

        assert!(item.is_some());
        let item = item.unwrap();

        assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
        assert_text_edit(item, "adist(");
        assert_has_parameter_hints(item);
    }

    #[test]
    fn test_reference_with_post_hoc_namespace() {
        let completions = static_items_with_test_package("debug(pkg::@adist)");
        let item = find_completion_by_label(&completions, "adist");

        assert!(item.is_some());
        let item = item.unwrap();

        assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
        assert_text_edit(item, "adist");
    }

    // Before sources are fetched the kind is unknown, but the existing name is
    // still consumed rather than inserted a second time
    #[test]
    fn test_post_hoc_namespace_before_sources_are_fetched() {
        let library = tempfile::TempDir::new().unwrap();
        let mut state = write_library_package(library.path(), "pkg", "export(adist)\n", None);

        for cursor_text in ["pkg::@adist", "pkg::@adist()", "debug(pkg::@adist)"] {
            let completions = static_items_with_state(cursor_text, &mut state);
            let item = find_completion_by_label(&completions, "adist").unwrap();

            assert_eq!(item.kind, Some(CompletionItemKind::STRUCT));
            assert_text_edit(item, "adist");
            assert_sort_text_has_priority_prefix(item);
            assert_eq!(item.preselect, Some(true));
        }
    }
}

#[cfg(test)]
mod empty_args_tests {
    use tower_lsp_server::ls_types::CompletionItemKind;

    use crate::lsp::completions::tests::utils::assert_has_parameter_hints;
    use crate::lsp::completions::tests::utils::assert_text_edit;
    use crate::lsp::completions::tests::utils::find_completion_by_label;
    use crate::lsp::completions::tests::utils::get_completions_at_cursor;
    use crate::lsp::completions::tests::utils::static_items_with_test_package;
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

            assert!(item.is_some());
            let item = item.unwrap();

            assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
            assert_text_edit(item, "abbreviate(");
            assert_has_parameter_hints(item);
        });
    }

    #[test]
    fn test_namespace_function_with_empty_parentheses() {
        let completions = static_items_with_test_package("pkg::@adist()");
        let item = find_completion_by_label(&completions, "adist");

        assert!(item.is_some());
        let item = item.unwrap();

        assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
        assert_text_edit(item, "adist(");
    }
}

#[cfg(test)]
mod nonempty_args_tests {
    use tower_lsp_server::ls_types::CompletionItemKind;

    use crate::lsp::completions::tests::utils::assert_no_command;
    use crate::lsp::completions::tests::utils::assert_text_edit;
    use crate::lsp::completions::tests::utils::find_completion_by_label;
    use crate::lsp::completions::tests::utils::get_completions_at_cursor;
    use crate::lsp::completions::tests::utils::static_items_with_test_package;
    use crate::r_task;

    #[test]
    fn test_function_name_with_nonempty_parentheses() {
        r_task(|| {
            let completions = get_completions_at_cursor("a@(\"hello\", width=10)").unwrap();
            let item = find_completion_by_label(&completions, "abbreviate");

            assert!(item.is_some());
            let item = item.unwrap();

            assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
            assert_text_edit(item, "abbreviate(");

            assert_no_command(item);
        });
    }

    #[test]
    fn test_namespace_function_with_nonempty_parentheses() {
        let completions = static_items_with_test_package("pkg::a@(\"hi\", \"bye\")");
        let item = find_completion_by_label(&completions, "adist");

        assert!(item.is_some());
        let item = item.unwrap();

        assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));
        assert_text_edit(item, "adist(");

        assert_no_command(item);
    }
}
