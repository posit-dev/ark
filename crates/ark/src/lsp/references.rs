//
// references.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::path::Path;

use anyhow::anyhow;
use ropey::Rope;
use stdext::unwrap::IntoResult;
use stdext::*;
use tower_lsp::lsp_types::Location;
use tower_lsp::lsp_types::Position;
use tower_lsp::lsp_types::Range;
use tower_lsp::lsp_types::ReferenceParams;
use tower_lsp::lsp_types::Url;
use tree_sitter::Node;
use tree_sitter::Point;
use walkdir::WalkDir;

use crate::lsp;
use crate::lsp::documents::Document;
use crate::lsp::encoding::convert_point_to_position;
use crate::lsp::encoding::convert_position_to_point;
use crate::lsp::indexer::filter_entry;
use crate::lsp::state::with_document;
use crate::lsp::state::WorldState;
use crate::lsp::traits::cursor::TreeCursorExt;
use crate::lsp::traits::rope::RopeExt;
use crate::lsp::traits::url::UrlExt;
use crate::treesitter::ExtractOperatorType;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

#[derive(Debug, PartialEq)]
enum ReferenceKind {
    SymbolName, // a regular R symbol
    DollarName, // a dollar name, following '$'
    AtName,     // a slot name, following '@'
}

// Assuming `x` is an `identifier`, is it the RHS of a `$` or `@`?
fn node_reference_kind(x: &Node) -> ReferenceKind {
    let Some(parent) = x.parent() else {
        // No `parent`, must be a regular symbol
        return ReferenceKind::SymbolName;
    };

    let parent_type = parent.node_type();

    if !matches!(parent_type, NodeType::ExtractOperator(_)) {
        // Parent not `$` or `@`
        return ReferenceKind::SymbolName;
    }

    // Need to check that we actually came from the RHS
    let Some(rhs) = parent.child_by_field_name("rhs") else {
        return ReferenceKind::SymbolName;
    };
    if &rhs != x {
        return ReferenceKind::SymbolName;
    };

    match parent_type {
        NodeType::ExtractOperator(ExtractOperatorType::Dollar) => ReferenceKind::DollarName,
        NodeType::ExtractOperator(ExtractOperatorType::At) => ReferenceKind::AtName,
        _ => std::unreachable!(),
    }
}

struct Context {
    kind: ReferenceKind,
    symbol: String,
}

fn add_reference(node: &Node, contents: &Rope, path: &Path, locations: &mut Vec<Location>) {
    let start = convert_point_to_position(contents, node.start_position());
    let end = convert_point_to_position(contents, node.end_position());

    let location = Location::new(
        Url::from_file_path(path).expect("valid path"),
        Range::new(start, end),
    );
    locations.push(location);
}

fn found_match(node: &Node, contents: &Rope, context: &Context) -> bool {
    if !node.is_identifier() {
        return false;
    }

    let symbol = contents.node_slice(node).unwrap().to_string();
    if symbol != context.symbol {
        return false;
    }

    context.kind == node_reference_kind(node)
}

fn build_context(uri: &Url, position: Position, state: &WorldState) -> anyhow::Result<Context> {
    // Unwrap the URL.
    let path = uri.file_path()?;

    // Figure out the identifier we're looking for.
    let context = with_document(path.as_path(), state, |document| {
        let ast = &document.ast;
        let contents = &document.contents;
        let point = convert_position_to_point(contents, position);

        let mut node = ast
            .root_node()
            .descendant_for_point_range(point, point)
            .into_result()?;

        // Check and see if we got an identifier. If we didn't, we might need to use
        // some heuristics to look around. Unfortunately, it seems like if you double-click
        // to select an identifier, and then use Right Click -> Find All References, the
        // position received by the LSP maps to the _end_ of the selected range, which
        // is technically not part of the associated identifier's range. In addition, we
        // can't just subtract 1 from the position column since that would then fail to
        // resolve the correct identifier when the cursor is located at the start of the
        // identifier.
        if !node.is_identifier() && point.column > 0 {
            let point = Point::new(point.row, point.column - 1);
            node = ast
                .root_node()
                .descendant_for_point_range(point, point)
                .into_result()?;
        }

        // double check that we found an identifier
        if !node.is_identifier() {
            return Err(anyhow!(
                "couldn't find an identifier associated with point {point:?}",
            ));
        }

        let kind = node_reference_kind(&node);

        // return identifier text contents
        let symbol = document.contents.node_slice(&node)?.to_string();

        Ok(Context { kind, symbol })
    });

    return context;
}

fn find_references_in_folder(
    context: &Context,
    path: &Path,
    locations: &mut Vec<Location>,
    state: &WorldState,
) {
    let walker = WalkDir::new(path);
    for entry in walker.into_iter().filter_entry(|entry| filter_entry(entry)) {
        let entry = unwrap!(entry, Err(_) => { continue; });
        let path = entry.path();
        let ext = unwrap!(path.extension(), None => { continue; });
        if ext != "r" && ext != "R" {
            continue;
        }

        lsp::log_info!("found R file {}", path.display());
        let result = with_document(path, state, |document| {
            find_references_in_document(context, path, document, locations);
            return Ok(());
        });

        match result {
            Ok(result) => result,
            Err(_error) => {
                lsp::log_warn!("error retrieving document for path {}", path.display());
                continue;
            },
        }
    }
}

fn find_references_in_document(
    context: &Context,
    path: &Path,
    document: &Document,
    locations: &mut Vec<Location>,
) {
    let ast = &document.ast;
    let contents = &document.contents;

    let mut cursor = ast.walk();
    cursor.recurse(|node| {
        if found_match(&node, contents, &context) {
            add_reference(&node, contents, path, locations);
        }

        return true;
    });
}

pub(crate) fn find_references(
    params: ReferenceParams,
    state: &WorldState,
) -> anyhow::Result<Vec<Location>> {
    // Create our locations vector.
    let mut locations: Vec<Location> = Vec::new();

    // Extract relevant parameters.
    let uri = params.text_document_position.text_document.uri;
    let position = params.text_document_position.position;

    // Figure out what we're looking for.
    let context = unwrap!(build_context(&uri, position, state), Err(err) => {
        return Err(anyhow!("Failed to find build context at position {position:?}: {err:?}"));
    });

    // Now, start searching through workspace folders for references to that identifier.
    for folder in state.workspace.folders.iter() {
        if let Ok(path) = folder.to_file_path() {
            lsp::log_info!("searching references in folder {}", path.display());
            find_references_in_folder(&context, &path, &mut locations, state);
        }
    }

    return Ok(locations);
}
