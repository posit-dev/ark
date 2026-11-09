//
// custom.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::syntax::sym_quote_invalid;
use harp::utils::r_typeof;
use libr::R_NilValue;
use libr::VECSXP;
use libr::VECTOR_ELT;
use stdext::unwrap;

use crate::lsp::call_arguments::active_parameter;
use crate::lsp::completions::completion_item::completion_item;
use crate::lsp::completions::completion_item::completion_item_from_dataset;
use crate::lsp::completions::completion_item::completion_item_from_package;
use crate::lsp::completions::request::ArgumentPosition;
use crate::lsp::completions::request::CustomRequest;
use crate::lsp::completions::request::SlotReply;
use crate::lsp::completions::sources::utils::set_sort_text_by_words_first;
use crate::lsp::completions::types::CompletionData;
use crate::lsp::signature_help::r_callee_formals;

/// Runs the custom completion callback registered for the callee and the
/// argument under the cursor. Declines when none is registered.
pub(super) fn r_custom_completions(request: &CustomRequest) -> anyhow::Result<SlotReply> {
    let CustomRequest {
        arguments,
        position,
        in_string,
    } = request;

    let Some(formals) = r_callee_formals(&arguments.callee)? else {
        return Ok(SlotReply::Decline);
    };
    let formals: Vec<String> = formals.into_iter().map(|argument| argument.name).collect();

    // TODO: Improve argument matching. `active_parameter()` approximates R's
    // matching rules and can fail to identify a formal under the cursor.
    // https://github.com/posit-dev/positron/issues/3467
    let Some(parameter) =
        active_parameter(arguments, &formals).and_then(|index| formals.get(index))
    else {
        log::trace!(
            "No formal of `{callee}` matches the argument under the cursor",
            callee = arguments.callee
        );
        return Ok(SlotReply::Decline);
    };

    let name = arguments.callee.as_str();

    let position = match position {
        ArgumentPosition::Name => "name",
        ArgumentPosition::Value => "value",
    };

    let mut completions = vec![];

    unsafe {
        // Call our custom completion function.
        let r_completions = RFunction::from(".ps.completions.getCustomCallCompletions")
            .param("name", name)
            .param("argument", parameter.as_str())
            .param("position", position)
            .call()?;

        if *r_completions == R_NilValue {
            // No custom completions detected. Let other sources provide results.
            return Ok(SlotReply::Decline);
        }

        if r_typeof(*r_completions) != VECSXP {
            // Weird internal issue, but we expected completions here so return
            // an empty set to signal that we are done
            return Ok(SlotReply::Fill(completions));
        }

        // TODO: Use safe access APIs here.
        let values = VECTOR_ELT(*r_completions, 0);
        let kind = VECTOR_ELT(*r_completions, 1);
        let enquote = VECTOR_ELT(*r_completions, 2);
        let append = VECTOR_ELT(*r_completions, 3);

        if let Ok(values) = RObject::view(values).to::<Vec<String>>() {
            let kind = RObject::view(kind)
                .to::<String>()
                .unwrap_or("unknown".to_string());

            let enquote = RObject::view(enquote).to::<bool>().unwrap_or(false);

            let append = RObject::view(append)
                .to::<String>()
                .unwrap_or("".to_string());

            for value in values.iter() {
                let value = value.clone();

                let item = match kind.as_str() {
                    "package" => completion_item_from_package(&value, false),
                    "dataset" => completion_item_from_dataset(&value),
                    _ => completion_item(&value, CompletionData::Unknown),
                };

                let mut item = unwrap!(item, Err(err) => {
                    log::error!("{err:?}");
                    continue;
                });

                if enquote && !in_string {
                    item.insert_text = Some(format!("\"{value}\""));
                } else {
                    let mut insert_text = sym_quote_invalid(value.as_str());

                    if !append.is_empty() {
                        insert_text = format!("{insert_text}{append}");
                    }

                    item.insert_text = Some(insert_text);
                }

                completions.push(item);
            }
        }
    }

    // In particular, push env vars that start with `_` to the end
    set_sort_text_by_words_first(&mut completions);

    Ok(SlotReply::Fill(completions))
}

#[cfg(test)]
mod tests {
    use tower_lsp_server::ls_types::CompletionItem;

    use crate::lsp::completions::r_completions::custom::r_custom_completions;
    use crate::lsp::completions::request::SlotReply;
    use crate::lsp::completions::tests::utils::request_at_cursor;
    use crate::r_task;

    fn custom_reply_at_cursor(code_with_cursor: &str) -> SlotReply {
        match request_at_cursor(code_with_cursor).custom {
            Some(request) => r_custom_completions(&request).unwrap(),
            None => SlotReply::Decline,
        }
    }

    fn custom_completions_at_cursor(code_with_cursor: &str) -> Vec<CompletionItem> {
        match custom_reply_at_cursor(code_with_cursor) {
            SlotReply::Fill(items) => items,
            reply => panic!("Expected custom completions at `{code_with_cursor}`, got {reply:?}"),
        }
    }

    // Helper functions for testing custom completions
    fn assert_has_completion(code_with_cursor: &str, name: &str, expected_insert_text: &str) {
        let completions = custom_completions_at_cursor(code_with_cursor);
        let completion = completions
            .into_iter()
            .find(|completion| completion.label == name);
        assert!(completion.is_some());

        let completion = completion.unwrap();
        let expected_text = expected_insert_text.replace("{name}", name);
        assert_eq!(completion.insert_text.unwrap(), expected_text);
    }

    fn assert_no_completions(code_with_cursor: &str) {
        assert_eq!(custom_reply_at_cursor(code_with_cursor), SlotReply::Decline);
    }

    #[test]
    fn test_completion_custom_library() {
        r_task(|| {
            let n_packages = {
                let n = harp::parse_eval_base("length(base::.packages(TRUE))").unwrap();
                let n = i32::try_from(n).unwrap();
                usize::try_from(n).unwrap()
            };

            let n_compls = custom_completions_at_cursor("library(@)").len();

            // There should be as many matches as installed packages
            assert_eq!(n_compls, n_packages);

            let compls = custom_completions_at_cursor("library(uti@)");

            assert!(compls.iter().any(|c| c.label == "utils"));
        })
    }

    #[test]
    fn test_completion_custom_sys_getenv() {
        r_task(|| {
            let name = "ARK_TEST_ENVVAR";
            harp::parse_eval_base(format!("Sys.setenv({name} = '1')").as_str()).unwrap();

            // Inside the parentheses
            assert_has_completion("Sys.getenv(@)", name, "\"{name}\"");

            // Inside the parentheses, multiline
            assert_has_completion("Sys.getenv(\n  @\n)", name, "\"{name}\"");

            // Named argument
            assert_has_completion("Sys.getenv(x = @)", name, "\"{name}\"");

            // Named argument, multiline
            assert_has_completion("Sys.getenv(\n  x = @\n)", name, "\"{name}\"");

            // Typed some and then requested completions
            assert_has_completion("Sys.getenv(ARK_@)", name, "\"{name}\"");

            // Typed some and then requested completions, multiline
            assert_has_completion("Sys.getenv(\n  ARK_@\n)", name, "\"{name}\"");

            // After a named argument
            assert_has_completion("Sys.getenv(unset = '1', @)", name, "\"{name}\"");

            // After a named argument, multiline
            assert_has_completion("Sys.getenv(\n  unset = '1',\n  @\n)", name, "\"{name}\"");

            // Should not have it here
            assert_no_completions("Sys.getenv('foo', @)");

            harp::parse_eval_base(format!("Sys.unsetenv('{name}')").as_str()).unwrap();
        })
    }

    #[test]
    fn test_completion_custom_sys_unsetenv() {
        r_task(|| {
            let name = "ARK_TEST_ENVVAR";
            harp::parse_eval_base(format!("Sys.setenv({name} = '1')").as_str()).unwrap();

            // Inside the parentheses
            assert_has_completion("Sys.unsetenv(@)", name, "\"{name}\"");

            // Named argument
            assert_has_completion("Sys.unsetenv(x = @)", name, "\"{name}\"");

            // Typed some and then requested completions
            assert_has_completion("Sys.unsetenv(ARK_@)", name, "\"{name}\"");

            // TODO: Technically `Sys.unsetenv()` takes a character vector, so we should probably provide
            // completions for this too, but it probably isn't that common in practice
            assert_no_completions("Sys.unsetenv(c(@))");

            harp::parse_eval_base(format!("Sys.unsetenv('{name}')").as_str()).unwrap();
        })
    }

    #[test]
    fn test_completion_custom_sys_setenv() {
        r_task(|| {
            let name = "ARK_TEST_ENVVAR";
            harp::parse_eval_base(format!("Sys.setenv({name} = '1')").as_str()).unwrap();

            // Inside the parentheses
            assert_has_completion("Sys.setenv(@)", name, "{name} = ");

            // Typed some and then requested completions
            assert_has_completion("Sys.setenv(ARK_@)", name, "{name} = ");

            // Should have it here too, this takes `...`
            assert_has_completion("Sys.setenv(foo = 'bar', @)", name, "{name} = ");

            harp::parse_eval_base(format!("Sys.unsetenv('{name}')").as_str()).unwrap();
        })
    }

    #[test]
    fn test_completion_custom_sys_setenv_value_position() {
        r_task(|| {
            // Single line, with space
            assert_no_completions("Sys.setenv(AAA = @)");

            // Single line, no space
            assert_no_completions("Sys.setenv(AAA =@)");

            // Multiline case, with space
            assert_no_completions("Sys.setenv(\n  AAA = @\n)");

            // Multiline case, no space
            assert_no_completions("Sys.setenv(\n  AAA =@\n)");
        })
    }

    #[test]
    fn test_completion_custom_get_option() {
        r_task(|| {
            let name = "ARK_TEST_OPTION";
            harp::parse_eval_base(format!("options({name} = '1')").as_str()).unwrap();

            // Inside the parentheses
            assert_has_completion("getOption(@)", name, "\"{name}\"");

            // Named argument
            assert_has_completion("getOption(x = @)", name, "\"{name}\"");

            // Typed some and then requested completions
            assert_has_completion("getOption(ARK_@)", name, "\"{name}\"");

            // After a named argument
            assert_has_completion("getOption(default = '1', @)", name, "\"{name}\"");

            // Should not have it here
            assert_no_completions("getOption('foo', @)");

            harp::parse_eval_base(format!("options({name} = NULL)").as_str()).unwrap();
        })
    }

    #[test]
    fn test_completion_custom_options() {
        r_task(|| {
            let name = "ARK_TEST_OPTION";
            harp::parse_eval_base(format!("options({name} = '1')").as_str()).unwrap();

            // Inside the parentheses
            assert_has_completion("options(@)", name, "{name} = ");

            // Typed some and then requested completions
            assert_has_completion("options(ARK_@)", name, "{name} = ");

            // Should have it here too, this takes `...`
            assert_has_completion("options(foo = 'bar', @)", name, "{name} = ");

            harp::parse_eval_base(format!("options({name} = NULL)").as_str()).unwrap();
        })
    }

    #[test]
    fn test_completion_custom_options_value_position() {
        r_task(|| {
            // Single line, with space
            assert_no_completions("options(AAA = @)");

            // Single line, no space
            assert_no_completions("options(AAA =@)");

            // Multiline case, with space
            assert_no_completions("options(\n  AAA = @\n)");

            // Multiline case, no space
            assert_no_completions("options(\n  AAA =@\n)");
        })
    }
}
