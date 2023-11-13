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
use libR_sys::R_NilValue;
use libR_sys::VECSXP;
use libR_sys::VECTOR_ELT;
use stdext::unwrap;
use stdext::IntoResult;
use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::completions::completion_item::completion_item;
use crate::lsp::completions::completion_item::completion_item_from_dataset;
use crate::lsp::completions::completion_item::completion_item_from_package;
use crate::lsp::completions::types::CompletionData;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::signature_help::signature_help;
use crate::lsp::traits::node::NodeExt;
use crate::lsp::traits::point::PointExt;
use crate::lsp::traits::tree::TreeExt;

pub fn completions_from_custom_source(
    context: &DocumentContext,
) -> Result<Option<Vec<CompletionItem>>> {
    // Use the signature help tools to figure out the necessary pieces.
    let position = context.point.as_position();

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
    //               ^^^^^^   ^^^^^
    //                name    value
    //
    // This is mainly relevant because we might only want to
    // provide certain completions in the 'name' position.
    let node = context.document.ast.node_at_point(context.point);

    let marker = node.bwd_leaf_iter().find_map(|node| match node.kind() {
        "(" | "comma" => Some("name"),
        "=" => Some("value"),
        _ => None,
    });

    let position = marker.unwrap_or("value");

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
                    item.insert_text = Some(format!("\"{}\"", value));
                } else if !append.is_empty() {
                    item.insert_text = Some(format!("{}{}", value, append));
                }

                completions.push(item);
            }
        }
    }

    Ok(Some(completions))
}
