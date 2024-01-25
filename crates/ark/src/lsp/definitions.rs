//
// definitions.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use anyhow::Result;
use tower_lsp::lsp_types::GotoDefinitionParams;
use tower_lsp::lsp_types::GotoDefinitionResponse;
use tower_lsp::lsp_types::LocationLink;
use tower_lsp::lsp_types::Range;
use tower_lsp::lsp_types::Url;

use crate::lsp::documents::Document;
use crate::lsp::encoding::convert_point_to_position;
use crate::lsp::encoding::convert_position_to_point;
use crate::lsp::indexer;
use crate::lsp::traits::node::NodeExt;
use crate::lsp::traits::rope::RopeExt;

pub unsafe fn goto_definition<'a>(
    document: &'a Document,
    params: GotoDefinitionParams,
) -> Result<Option<GotoDefinitionResponse>> {
    // get reference to AST
    let ast = &document.ast;

    let contents = &document.contents;

    // try to find node at position
    let position = params.text_document_position_params.position;
    let point = convert_position_to_point(contents, position);

    let Some(node) = ast.root_node().find_closest_node_to_point(point) else {
        log::warn!("Failed to find the closest node to point {point}.");
        return Ok(None);
    };

    let start = convert_point_to_position(contents, node.start_position());
    let end = convert_point_to_position(contents, node.end_position());
    let range = Range { start, end };

    // search for a reference in the document index
    if matches!(node.kind(), "identifier") {
        let symbol = document.contents.node_slice(&node)?.to_string();
        if let Some((path, entry)) = indexer::find(symbol.as_str()) {
            let link = LocationLink {
                origin_selection_range: None,
                target_uri: Url::from_file_path(path).unwrap(),
                target_range: entry.range,
                target_selection_range: entry.range,
            };
            let response = GotoDefinitionResponse::Link(vec![link]);
            return Ok(Some(response));
        }
    }

    // TODO: We should see if we can find the referenced item in:
    //
    // 1. The document's current AST,
    // 2. The public functions from other documents in the project,
    // 3. A definition in the R session (which we could open in a virtual document)
    //
    // If we can't find a definition, then we can return the referenced item itself,
    // which will tell Positron to instead try to look for references for that symbol.
    let link = LocationLink {
        origin_selection_range: Some(range),
        target_uri: params.text_document_position_params.text_document.uri,
        target_range: range,
        target_selection_range: range,
    };

    let response = GotoDefinitionResponse::Link(vec![link]);
    Ok(Some(response))
}
