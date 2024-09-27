//
// custom.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::Result;
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
use tower_lsp::lsp_types::CompletionItem;

use crate::lsp;
use crate::lsp::completions::completion_item::completion_item;
use crate::lsp::completions::completion_item::completion_item_from_dataset;
use crate::lsp::completions::completion_item::completion_item_from_package;
use crate::lsp::completions::sources::utils::call_node_position_type;
use crate::lsp::completions::sources::utils::set_sort_text_by_words_first;
use crate::lsp::completions::sources::utils::CallNodePositionType;
use crate::lsp::completions::types::CompletionData;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::signature_help::r_signature_help;
use crate::treesitter::node_in_string;
use crate::treesitter::NodeTypeExt;

pub fn completions_from_custom_source(
    context: &DocumentContext,
) -> Result<Option<Vec<CompletionItem>>> {
    log::info!("completions_from_custom_source()");

    let mut node = context.node;

    let mut has_call = false;

    loop {
        // Try custom call completions
        if node.is_call() {
            has_call = true;
            break;
        }

        // If we reach a brace list, bail.
        if node.is_braced_expression() {
            break;
        }

        // Update the node.
        node = match node.parent() {
            Some(node) => node,
            None => break,
        };
    }

    if !has_call {
        // Didn't detect anything worth completing in this context,
        // let other sources add their own candidates instead
        return Ok(None);
    }

    completions_from_custom_source_impl(context)
}

pub fn completions_from_custom_source_impl(
    context: &DocumentContext,
) -> Result<Option<Vec<CompletionItem>>> {
    let point = context.point;
    let node = context.node;

    // Use the signature help tools to figure out the necessary pieces.
    let signatures = r_signature_help(context)?;
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
        tower_lsp::lsp_types::ParameterLabel::LabelOffsets([start, end]) => {
            let label = signature.label.as_str();
            let substring = label.get((start as usize)..(end as usize));
            substring.unwrap().to_string()
        },
        tower_lsp::lsp_types::ParameterLabel::Simple(string) => string,
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
    use tree_sitter::Point;

    use crate::lsp::completions::sources::unique::custom::completions_from_custom_source;
    use crate::lsp::document_context::DocumentContext;
    use crate::lsp::documents::Document;
    use crate::fixtures::point_from_cursor;
    use crate::fixtures::r_test;

    #[test]
    fn test_completion_custom_library() {
        r_test(|| {
            let n_packages = {
                let n = harp::parse_eval_base("length(base::.packages(TRUE))").unwrap();
                let n = i32::try_from(n).unwrap();
                usize::try_from(n).unwrap()
            };

            let point = Point { row: 0, column: 8 };
            let document = Document::new("library()", None);
            let context = DocumentContext::new(&document, point, None);

            let n_compls = completions_from_custom_source(&context)
                .unwrap()
                .unwrap()
                .len();

            // There should be as many matches as installed packages
            assert_eq!(n_compls, n_packages);

            let point = Point { row: 0, column: 11 };
            let document = Document::new("library(uti)", None);
            let context = DocumentContext::new(&document, point, None);

            let compls = completions_from_custom_source(&context).unwrap().unwrap();

            assert!(compls.iter().any(|c| c.label == "utils"));
        })
    }

    #[test]
    fn test_completion_custom_sys_getenv() {
        r_test(|| {
            let name = "ARK_TEST_ENVVAR";

            harp::parse_eval_base(format!("Sys.setenv({name} = '1')").as_str()).unwrap();

            let assert_has_ark_test_envvar_completion = |text: &str, point: Point| {
                let document = Document::new(text, None);
                let context = DocumentContext::new(&document, point, None);

                let completions = completions_from_custom_source(&context).unwrap().unwrap();
                let completion = completions
                    .into_iter()
                    .find(|completion| completion.label == name);
                assert!(completion.is_some());

                // Insert text is quoted!
                let completion = completion.unwrap();
                assert_eq!(completion.insert_text.unwrap(), format!("\"{name}\""));
            };

            // Inside the parentheses
            let (text, point) = point_from_cursor("Sys.getenv(@)");
            assert_has_ark_test_envvar_completion(text.as_str(), point);

            // Named argument
            let (text, point) = point_from_cursor("Sys.getenv(x = @)");
            assert_has_ark_test_envvar_completion(text.as_str(), point);

            // Typed some and then requested completions
            let (text, point) = point_from_cursor("Sys.getenv(ARK_@)");
            assert_has_ark_test_envvar_completion(text.as_str(), point);

            // After a named argument
            let (text, point) = point_from_cursor("Sys.getenv(unset = '1', @)");
            assert_has_ark_test_envvar_completion(text.as_str(), point);

            // Should not have it here
            let (text, point) = point_from_cursor("Sys.getenv('foo', @)");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let completions = completions_from_custom_source(&context).unwrap();
            assert!(completions.is_none());

            harp::parse_eval_base(format!("Sys.unsetenv('{name}')").as_str()).unwrap();
        })
    }

    #[test]
    fn test_completion_custom_sys_unsetenv() {
        r_test(|| {
            let name = "ARK_TEST_ENVVAR";

            harp::parse_eval_base(format!("Sys.setenv({name} = '1')").as_str()).unwrap();

            let assert_has_ark_test_envvar_completion = |text: &str, point: Point| {
                let document = Document::new(text, None);
                let context = DocumentContext::new(&document, point, None);

                let completions = completions_from_custom_source(&context).unwrap().unwrap();
                let completion = completions
                    .into_iter()
                    .find(|completion| completion.label == name);
                assert!(completion.is_some());

                // Insert text is quoted!
                let completion = completion.unwrap();
                assert_eq!(completion.insert_text.unwrap(), format!("\"{name}\""));
            };

            // Inside the parentheses
            let (text, point) = point_from_cursor("Sys.unsetenv(@)");
            assert_has_ark_test_envvar_completion(text.as_str(), point);

            // Named argument
            let (text, point) = point_from_cursor("Sys.unsetenv(x = @)");
            assert_has_ark_test_envvar_completion(text.as_str(), point);

            // Typed some and then requested completions
            let (text, point) = point_from_cursor("Sys.unsetenv(ARK_@)");
            assert_has_ark_test_envvar_completion(text.as_str(), point);

            // TODO: Technically `Sys.unsetenv()` takes a character vector, so we should probably provide
            // completions for this too, but it probably isn't that common in practice
            let (text, point) = point_from_cursor("Sys.unsetenv(c(@))");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let completions = completions_from_custom_source(&context).unwrap();
            assert!(completions.is_none());

            harp::parse_eval_base(format!("Sys.unsetenv('{name}')").as_str()).unwrap();
        })
    }

    #[test]
    fn test_completion_custom_sys_setenv() {
        r_test(|| {
            let name = "ARK_TEST_ENVVAR";

            harp::parse_eval_base(format!("Sys.setenv({name} = '1')").as_str()).unwrap();

            let assert_has_ark_test_envvar_completion = |text: &str, point: Point| {
                let document = Document::new(text, None);
                let context = DocumentContext::new(&document, point, None);

                let completions = completions_from_custom_source(&context).unwrap().unwrap();
                let completion = completions
                    .into_iter()
                    .find(|completion| completion.label == name);
                assert!(completion.is_some());

                // Insert text is NOT quoted! And we get an ` = ` appended.
                let completion = completion.unwrap();
                assert_eq!(completion.insert_text.unwrap(), format!("{name} = "));
            };

            // Inside the parentheses
            let (text, point) = point_from_cursor("Sys.setenv(@)");
            assert_has_ark_test_envvar_completion(text.as_str(), point);

            // Typed some and then requested completions
            let (text, point) = point_from_cursor("Sys.setenv(ARK_@)");
            assert_has_ark_test_envvar_completion(text.as_str(), point);

            // Should have it here too, this takes `...`
            let (text, point) = point_from_cursor("Sys.setenv(foo = 'bar', @)");
            assert_has_ark_test_envvar_completion(text.as_str(), point);

            harp::parse_eval_base(format!("Sys.unsetenv('{name}')").as_str()).unwrap();
        })
    }

    #[test]
    fn test_completion_custom_get_option() {
        r_test(|| {
            let name = "ARK_TEST_OPTION";

            harp::parse_eval_base(format!("options({name} = '1')").as_str()).unwrap();

            let assert_has_ark_test_envvar_completion = |text: &str, point: Point| {
                let document = Document::new(text, None);
                let context = DocumentContext::new(&document, point, None);

                let completions = completions_from_custom_source(&context).unwrap().unwrap();
                let completion = completions
                    .into_iter()
                    .find(|completion| completion.label == name);
                assert!(completion.is_some());

                // Insert text is quoted!
                let completion = completion.unwrap();
                assert_eq!(completion.insert_text.unwrap(), format!("\"{name}\""));
            };

            // Inside the parentheses
            let (text, point) = point_from_cursor("getOption(@)");
            assert_has_ark_test_envvar_completion(text.as_str(), point);

            // Named argument
            let (text, point) = point_from_cursor("getOption(x = @)");
            assert_has_ark_test_envvar_completion(text.as_str(), point);

            // Typed some and then requested completions
            let (text, point) = point_from_cursor("getOption(ARK_@)");
            assert_has_ark_test_envvar_completion(text.as_str(), point);

            // After a named argument
            let (text, point) = point_from_cursor("getOption(default = '1', @)");
            assert_has_ark_test_envvar_completion(text.as_str(), point);

            // Should not have it here
            let (text, point) = point_from_cursor("getOption('foo', @)");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let completions = completions_from_custom_source(&context).unwrap();
            assert!(completions.is_none());

            harp::parse_eval_base(format!("options({name} = NULL)").as_str()).unwrap();
        })
    }

    #[test]
    fn test_completion_custom_options() {
        r_test(|| {
            let name = "ARK_TEST_OPTION";

            harp::parse_eval_base(format!("options({name} = '1')").as_str()).unwrap();

            let assert_has_ark_test_option_completion = |text: &str, point: Point| {
                let document = Document::new(text, None);
                let context = DocumentContext::new(&document, point, None);

                let completions = completions_from_custom_source(&context).unwrap().unwrap();
                let completion = completions
                    .into_iter()
                    .find(|completion| completion.label == name);
                assert!(completion.is_some());

                // Insert text is NOT quoted! And we get an ` = ` appended.
                let completion = completion.unwrap();
                assert_eq!(completion.insert_text.unwrap(), format!("{name} = "));
            };

            // Inside the parentheses
            let (text, point) = point_from_cursor("options(@)");
            assert_has_ark_test_option_completion(text.as_str(), point);

            // Typed some and then requested completions
            let (text, point) = point_from_cursor("options(ARK_@)");
            assert_has_ark_test_option_completion(text.as_str(), point);

            // Should have it here too, this takes `...`
            let (text, point) = point_from_cursor("options(foo = 'bar', @)");
            assert_has_ark_test_option_completion(text.as_str(), point);

            harp::parse_eval_base(format!("options({name} = NULL)").as_str()).unwrap();
        })
    }
}
