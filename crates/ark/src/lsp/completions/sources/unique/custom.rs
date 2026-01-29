//
// custom.rs
//
// Copyright (C) 2023-2025 Posit Software, PBC. All rights reserved.
//
//

use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::utils::r_typeof;
use harp::utils::sym_quote_invalid;
use libr::R_NilValue;
use libr::VECSXP;
use libr::VECTOR_ELT;
use stdext::unwrap;
use stdext::IntoResult;
use tower_lsp::lsp_types;
use tower_lsp::lsp_types::CompletionItem;

use crate::lsp;
use crate::lsp::completions::completion_context::CompletionContext;
use crate::lsp::completions::completion_item::completion_item;
use crate::lsp::completions::completion_item::completion_item_from_dataset;
use crate::lsp::completions::completion_item::completion_item_from_package;
use crate::lsp::completions::sources::utils::call_node_position_type;
use crate::lsp::completions::sources::utils::set_sort_text_by_words_first;
use crate::lsp::completions::sources::utils::CallNodePositionType;
use crate::lsp::completions::sources::CompletionSource;
use crate::lsp::completions::types::CompletionData;
use crate::lsp::signature_help::r_signature_help;
use crate::treesitter::node_in_string;
pub(super) struct CustomSource;

impl CompletionSource for CustomSource {
    fn name(&self) -> &'static str {
        "custom"
    }

    fn provide_completions(
        &self,
        completion_context: &CompletionContext,
    ) -> anyhow::Result<Option<Vec<CompletionItem>>> {
        completions_from_custom_source(completion_context)
    }
}

fn completions_from_custom_source(
    context: &CompletionContext,
) -> anyhow::Result<Option<Vec<CompletionItem>>> {
    if context.containing_call_node().is_none() {
        // Didn't detect anything worth completing in this context,
        // let other sources add their own candidates instead
        return Ok(None);
    }

    let document_context = context.document_context;
    let point = document_context.point;
    let node = document_context.node;

    // Use the signature help tools to figure out the necessary pieces.
    let signatures = r_signature_help(document_context)?;
    let Some(signatures) = signatures else {
        return Ok(None);
    };

    // Pull out the relevant signature information.
    let signature = signatures.signatures.get(0).into_result()?;
    let mut name = signature.label.clone();
    let parameters = signature.parameters.as_ref().into_result()?;
    let index = signature.active_parameter.into_result()? as usize;
    // TODO: Currently, argument matching is not very accurate. This is just a
    // workaround to supresses the error rather than showing a cryptic error
    // message to users, but there should be some better option.
    //
    // cf. https://github.com/posit-dev/positron/issues/3467
    if index >= parameters.len() {
        lsp::log_error!("Index {index} is out of bounds of the parameters of `{name}`");
        return Ok(None);
    }
    let parameter = parameters.get(index).into_result()?;

    // Extract the parameter text.
    let parameter = match parameter.label.clone() {
        lsp_types::ParameterLabel::LabelOffsets([start, end]) => {
            let label = signature.label.as_str();
            let substring = label.get((start as usize)..(end as usize));
            substring.unwrap().to_string()
        },
        lsp_types::ParameterLabel::Simple(string) => string,
    };

    // Parameter text typically contains the parameter name and its default value if there is one.
    // Extract out just the parameter name for matching purposes.
    let parameter = match parameter.find("=") {
        Some(loc) => &parameter[..loc].trim(),
        None => parameter.as_str(),
    };

    // Trim off the function parameters from the signature.
    if let Some(index) = name.find('(') {
        name = name[0..index].to_string();
    }

    // Check and see if we're in the 'name' position,
    // versus the 'value' position, for a function invocation.
    //
    // For example:
    //
    //    Sys.setenv(EDITOR = "vim")
    //    ^^^^^^^^^^ ^^^^^^   ^^^^^ ^
    //    other      name     value other
    //
    // This is mainly relevant because we might only want to
    // provide certain completions in the 'name' position.
    let position = match call_node_position_type(&node, point) {
        CallNodePositionType::Name => "name",
        // Currently mapping ambiguous `fn(arg<tab>)` to `"name"`, but we could
        // return `"ambiguous"` and allow our handlers to handle this individually
        CallNodePositionType::Ambiguous => "name",
        CallNodePositionType::Value => "value",
        CallNodePositionType::Outside => {
            // Call detected, but on the RHS of a `)` node or the LHS
            // of a `(` node, i.e. outside the parenthesis.
            return Ok(None);
        },
        CallNodePositionType::Unknown => {
            // Call detected, but inside some very odd edge case
            return Ok(None);
        },
    };

    let mut completions = vec![];

    unsafe {
        // Call our custom completion function.
        let r_completions = RFunction::from(".ps.completions.getCustomCallCompletions")
            .param("name", name)
            .param("argument", parameter)
            .param("position", position)
            .call()?;

        if *r_completions == R_NilValue {
            // No custom completions detected. Let other sources provide results.
            return Ok(None);
        }

        if r_typeof(*r_completions) != VECSXP {
            // Weird internal issue, but we expected completions here so return
            // an empty set to signal that we are done
            return Ok(Some(completions));
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

                if enquote && !node_in_string(&node) {
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

    Ok(Some(completions))
}

#[cfg(test)]
mod tests {
    use crate::fixtures::point_from_cursor;
    use crate::lsp::completions::completion_context::CompletionContext;
    use crate::lsp::completions::sources::unique::custom::completions_from_custom_source;
    use crate::lsp::document_context::DocumentContext;
    use crate::lsp::document::Document;
    use crate::lsp::state::WorldState;
    use crate::r_task;

    // Helper functions for testing custom completions
    fn assert_has_completion(code_with_cursor: &str, name: &str, expected_insert_text: &str) {
        let (text, point) = point_from_cursor(code_with_cursor);
        let state = WorldState::default();
        let document = Document::new(text.as_str(), None);
        let document_context = DocumentContext::new(&document, point, None);
        let context = CompletionContext::new(&document_context, &state);

        let completions = completions_from_custom_source(&context).unwrap().unwrap();
        let completion = completions
            .into_iter()
            .find(|completion| completion.label == name);
        assert!(completion.is_some());

        let completion = completion.unwrap();
        let expected_text = expected_insert_text.replace("{name}", name);
        assert_eq!(completion.insert_text.unwrap(), expected_text);
    }

    fn assert_no_completions(code_with_cursor: &str) {
        let (text, point) = point_from_cursor(code_with_cursor);
        let state = WorldState::default();
        let document = Document::new(text.as_str(), None);
        let document_context = DocumentContext::new(&document, point, None);
        let context = CompletionContext::new(&document_context, &state);

        let completions = completions_from_custom_source(&context).unwrap();
        assert!(completions.is_none());
    }

    #[test]
    fn test_completion_custom_library() {
        r_task(|| {
            let n_packages = {
                let n = harp::parse_eval_base("length(base::.packages(TRUE))").unwrap();
                let n = i32::try_from(n).unwrap();
                usize::try_from(n).unwrap()
            };

            let (text, point) = point_from_cursor("library(@)");
            let state = WorldState::default();
            let document = Document::new(text.as_str(), None);
            let document_context = DocumentContext::new(&document, point, None);
            let context = CompletionContext::new(&document_context, &state);

            let n_compls = completions_from_custom_source(&context)
                .unwrap()
                .unwrap()
                .len();

            // There should be as many matches as installed packages
            assert_eq!(n_compls, n_packages);

            let (text, point) = point_from_cursor("library(uti@)");
            let state = WorldState::default();
            let document = Document::new(text.as_str(), None);
            let document_context = DocumentContext::new(&document, point, None);
            let context = CompletionContext::new(&document_context, &state);

            let compls = completions_from_custom_source(&context).unwrap().unwrap();

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
