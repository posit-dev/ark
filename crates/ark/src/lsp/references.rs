//
// references.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::path::Path;

use anyhow::bail;
use log::info;
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

use crate::lsp::backend::Backend;
use crate::lsp::documents::Document;
use crate::lsp::encoding::convert_point_to_position;
use crate::lsp::encoding::convert_position_to_point;
use crate::lsp::indexer::filter_entry;
use crate::lsp::traits::cursor::TreeCursorExt;
use crate::lsp::traits::rope::RopeExt;
use crate::lsp::traits::url::UrlExt;
use crate::treesitter::NodeTypeExt;

enum ReferenceKind {
    SymbolName, // a regular R symbol
    DollarName, // a dollar name, following '$'
    SlotName,   // a slot name, following '@'
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

    match context.kind {
        ReferenceKind::DollarName => {
            if let Some(sibling) = node.prev_sibling() {
                if sibling.kind() == "$" {
                    return true;
                }
            }
        },

        ReferenceKind::SlotName => {
            if let Some(sibling) = node.prev_sibling() {
                if sibling.kind() == "@" {
                    return true;
                }
            }
        },

        ReferenceKind::SymbolName => {
            if let Some(sibling) = node.prev_sibling() {
                if !matches!(sibling.kind(), "$" | "@") {
                    return true;
                }
            } else {
                return true;
            }
        },
    }

    return false;
}

impl Backend {
    fn build_context(&self, uri: &Url, position: Position) -> anyhow::Result<Context> {
        // Unwrap the URL.
        let path = uri.file_path()?;

        // Figure out the identifier we're looking for.
        let context = self.with_document(path.as_path(), |document| {
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
                bail!(
                    "couldn't find an identifier associated with point {:?}",
                    point
                );
            }

            // check this node's previous sibling; if this is the name of a slot
            // or dollar accessed item, we should mark it
            let kind = match node.prev_sibling() {
                None => {
                    info!("node {:?} has no previous sibling", node);
                    ReferenceKind::SymbolName
                },

                Some(sibling) => {
                    info!("found sibling {:?} ({})", sibling, sibling.kind());
                    match sibling.kind() {
                        "$" => ReferenceKind::DollarName,
                        "@" => ReferenceKind::SlotName,
                        _ => ReferenceKind::SymbolName,
                    }
                },
            };

            // return identifier text contents
            let symbol = document.contents.node_slice(&node)?.to_string();

            Ok(Context { kind, symbol })
        });

        return context;
    }

    fn find_references_in_folder(
        &self,
        context: &Context,
        path: &Path,
        locations: &mut Vec<Location>,
    ) {
        let walker = WalkDir::new(path);
        for entry in walker.into_iter().filter_entry(|entry| filter_entry(entry)) {
            let entry = unwrap!(entry, Err(_) => { continue; });
            let path = entry.path();
            let ext = unwrap!(path.extension(), None => { continue; });
            if ext != "r" && ext != "R" {
                continue;
            }

            info!("found R file {}", path.display());
            let result = self.with_document(path, |document| {
                self.find_references_in_document(context, path, document, locations);
                return Ok(());
            });

            match result {
                Ok(result) => result,
                Err(_error) => {
                    info!("error retrieving document for path {}", path.display());
                    continue;
                },
            }
        }
    }

    fn find_references_in_document(
        &self,
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

    pub fn find_references(&self, params: ReferenceParams) -> Result<Vec<Location>, ()> {
        // Create our locations vector.
        let mut locations: Vec<Location> = Vec::new();

        // Extract relevant parameters.
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        // Figure out what we're looking for.
        let context = unwrap!(self.build_context(&uri, position), Err(_) => {
            info!("failed to find build context at position {:?}", position);
            return Err(());
        });

        // Now, start searching through workspace folders for references to that identifier.
        let workspace = self.workspace.lock();
        for folder in workspace.folders.iter() {
            if let Ok(path) = folder.to_file_path() {
                info!("searching references in folder {}", path.display());
                self.find_references_in_folder(&context, &path, &mut locations);
            }
        }

        return Ok(locations);
    }
}
