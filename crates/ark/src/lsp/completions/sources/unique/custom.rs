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
use harp::utils::r_symbol_quote_invalid;
use harp::utils::r_typeof;
use libR_shim::R_NilValue;
use libR_shim::VECSXP;
use libR_shim::VECTOR_ELT;
use stdext::unwrap;
use stdext::IntoResult;
use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::completions::completion_item::completion_item;
use crate::lsp::completions::completion_item::completion_item_from_dataset;
use crate::lsp::completions::completion_item::completion_item_from_package;
use crate::lsp::completions::sources::utils::call_node_position_type;
use crate::lsp::completions::sources::utils::set_sort_text_by_words_first;
use crate::lsp::completions::sources::utils::CallNodePositionType;
use crate::lsp::completions::types::CompletionData;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::signature_help::signature_help;
use crate::lsp::traits::point::PointExt;

pub fn completions_from_custom_source(
    context: &DocumentContext,
) -> Result<Option<Vec<CompletionItem>>> {
    log::info!("completions_from_custom_source()");

    let mut node = context.node;

    let mut has_call = false;

    loop {
        // Try custom call completions
        if node.kind() == "call" {
            has_call = true;
            break;
        }

        // If we reach a brace list, bail.
        if node.kind() == "{" {
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
    let position = point.as_position();

    let signatures = unsafe { signature_help(context.document, &position)? };
    let Some(signatures) = signatures else {
        return Ok(None);
    };

    // Pull out the relevant signature information.
    let signature = signatures.signatures.get(0).into_result()?;
    let mut name = signature.label.clone();
    let parameters = signature.parameters.as_ref().into_result()?;
    let index = signature.active_parameter.into_result()?;
    let parameter = parameters.get(index as usize).into_result()?;

    // Extract the argument text.
    let argument = match parameter.label.clone() {
        tower_lsp::lsp_types::ParameterLabel::LabelOffsets([start, end]) => {
            let label = signature.label.as_str();
            let substring = label.get((start as usize)..(end as usize));
            substring.unwrap().to_string()
        },
        tower_lsp::lsp_types::ParameterLabel::Simple(string) => string,
    };

    // Trim off the function arguments from the signature.
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
            .param("argument", argument)
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

                if enquote && node.kind() != "string" {
                    item.insert_text = Some(format!("\"{value}\""));
                } else {
                    let mut insert_text = r_symbol_quote_invalid(value.as_str());

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
    use harp::environment::R_ENVS;
    use harp::eval::r_parse_eval0;
    use tree_sitter::Point;

    use crate::lsp::completions::sources::unique::custom::completions_from_custom_source_impl;
    use crate::lsp::document_context::DocumentContext;
    use crate::lsp::documents::Document;
    use crate::test::r_test;

    #[test]
    fn test_completion_custom_library() {
        r_test(|| {
            let n_packages = {
                let n = r_parse_eval0("length(base::.packages(TRUE))", R_ENVS.global).unwrap();
                let n = i32::try_from(n).unwrap();
                usize::try_from(n).unwrap()
            };

            let point = Point { row: 0, column: 8 };
            let document = Document::new("library()");
            let context = DocumentContext::new(&document, point, None);

            let n_compls = completions_from_custom_source_impl(&context)
                .unwrap()
                .unwrap()
                .len();

            // There should be as many matches as installed packages
            assert_eq!(n_compls, n_packages);

            let point = Point { row: 0, column: 11 };
            let document = Document::new("library(uti)");
            let context = DocumentContext::new(&document, point, None);

            let compls = completions_from_custom_source_impl(&context)
                .unwrap()
                .unwrap();

            assert!(compls.iter().any(|c| c.label == "utils"));
        })
    }
}
