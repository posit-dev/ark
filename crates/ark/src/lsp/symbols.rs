//
// symbols.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

#![allow(deprecated)]

use std::result::Result::Ok;

use anyhow::*;
use log::*;
use ropey::Rope;
use stdext::unwrap::IntoResult;
use tower_lsp::lsp_types::DocumentSymbol;
use tower_lsp::lsp_types::DocumentSymbolParams;
use tower_lsp::lsp_types::Location;
use tower_lsp::lsp_types::Range;
use tower_lsp::lsp_types::SymbolInformation;
use tower_lsp::lsp_types::SymbolKind;
use tower_lsp::lsp_types::Url;
use tower_lsp::lsp_types::WorkspaceSymbolParams;
use tree_sitter::Node;

use crate::lsp::backend::Backend;
use crate::lsp::encoding::convert_point_to_position;
use crate::lsp::indexer;
use crate::lsp::indexer::IndexEntryData;
use crate::lsp::traits::rope::RopeExt;
use crate::lsp::traits::string::StringExt;
use crate::treesitter::BinaryOperatorType;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

pub fn symbols(
    _backend: &Backend,
    params: &WorkspaceSymbolParams,
) -> Result<Vec<SymbolInformation>> {
    let query = &params.query;
    let mut info: Vec<SymbolInformation> = Vec::new();

    indexer::map(|path, symbol, entry| {
        if !symbol.fuzzy_matches(query) {
            return;
        }

        match &entry.data {
            IndexEntryData::Function { name, arguments: _ } => {
                info.push(SymbolInformation {
                    name: name.to_string(),
                    kind: SymbolKind::FUNCTION,
                    location: Location {
                        uri: Url::from_file_path(path).unwrap(),
                        range: entry.range,
                    },
                    tags: None,
                    deprecated: None,
                    container_name: None,
                });
            },

            IndexEntryData::Section { level: _, title } => {
                info.push(SymbolInformation {
                    name: title.to_string(),
                    kind: SymbolKind::MODULE,
                    location: Location {
                        uri: Url::from_file_path(path).unwrap(),
                        range: entry.range,
                    },
                    tags: None,
                    deprecated: None,
                    container_name: None,
                });
            },
        };
    });

    Ok(info)
}

pub fn document_symbols(
    backend: &Backend,
    params: &DocumentSymbolParams,
) -> Result<Vec<DocumentSymbol>> {
    let mut symbols: Vec<DocumentSymbol> = Vec::new();

    let uri = &params.text_document.uri;
    let document = backend.state.documents.get(uri).into_result()?;
    let ast = &document.ast;
    let contents = &document.contents;

    let node = ast.root_node();

    let start = convert_point_to_position(contents, node.start_position());
    let end = convert_point_to_position(contents, node.end_position());

    // construct a root symbol, so we always have something to append to
    let mut root = DocumentSymbol {
        name: "<root>".to_string(),
        kind: SymbolKind::NULL,
        children: Some(Vec::new()),
        deprecated: None,
        tags: None,
        detail: None,
        range: Range { start, end },
        selection_range: Range { start, end },
    };

    // index from the root
    index_node(&node, &contents, &mut root, &mut symbols)?;

    // return the children we found
    Ok(root.children.unwrap_or_default())
}

fn is_indexable(node: &Node) -> bool {
    // don't index 'arguments' or 'parameters'
    if matches!(node.node_type(), NodeType::Arguments | NodeType::Parameters) {
        return false;
    }

    true
}

fn index_node(
    node: &Node,
    contents: &Rope,
    parent: &mut DocumentSymbol,
    symbols: &mut Vec<DocumentSymbol>,
) -> Result<bool> {
    // if we find an assignment, index it
    if matches!(
        node.node_type(),
        NodeType::BinaryOperator(BinaryOperatorType::LeftAssignment) |
            NodeType::BinaryOperator(BinaryOperatorType::EqualsAssignment)
    ) {
        match index_assignment(node, contents, parent, symbols) {
            Ok(handled) => {
                if handled {
                    return Ok(true);
                }
            },
            Err(error) => error!("{:?}", error),
        }
    }

    // by default, recurse into children
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if is_indexable(&child) {
            let result = index_node(&child, contents, parent, symbols);
            if let Err(error) = result {
                error!("{:?}", error);
            }
        }
    }

    Ok(true)
}

fn index_assignment(
    node: &Node,
    contents: &Rope,
    parent: &mut DocumentSymbol,
    symbols: &mut Vec<DocumentSymbol>,
) -> Result<bool> {
    // check for assignment
    matches!(
        node.node_type(),
        NodeType::BinaryOperator(BinaryOperatorType::LeftAssignment) |
            NodeType::BinaryOperator(BinaryOperatorType::EqualsAssignment)
    )
    .into_result()?;

    // check for lhs, rhs
    let lhs = node.child_by_field_name("lhs").into_result()?;
    let rhs = node.child_by_field_name("rhs").into_result()?;

    // check for identifier on lhs, function on rhs
    let function = lhs.is_identifier_or_string() && rhs.is_function_definition();

    if function {
        return index_assignment_with_function(node, contents, parent, symbols);
    }

    // otherwise, just index as generic object
    let name = contents.node_slice(&lhs)?.to_string();

    let start = convert_point_to_position(contents, lhs.start_position());
    let end = convert_point_to_position(contents, lhs.end_position());

    let symbol = DocumentSymbol {
        name,
        kind: SymbolKind::OBJECT,
        detail: None,
        children: Some(Vec::new()),
        deprecated: None,
        tags: None,
        range: Range::new(start, end),
        selection_range: Range::new(start, end),
    };

    // add this symbol to the parent node
    parent.children.as_mut().unwrap().push(symbol);

    Ok(true)
}

fn index_assignment_with_function(
    node: &Node,
    contents: &Rope,
    parent: &mut DocumentSymbol,
    symbols: &mut Vec<DocumentSymbol>,
) -> Result<bool> {
    // check for lhs, rhs
    let lhs = node.child_by_field_name("lhs").into_result()?;
    let rhs = node.child_by_field_name("rhs").into_result()?;

    // start extracting the argument names
    let mut arguments: Vec<String> = Vec::new();
    let parameters = rhs.child_by_field_name("parameters").into_result()?;

    let mut cursor = parameters.walk();
    for parameter in parameters.children_by_field_name("parameter", &mut cursor) {
        let name = parameter.child_by_field_name("name").into_result()?;
        let name = contents.node_slice(&name)?.to_string();
        arguments.push(name);
    }

    let name = contents.node_slice(&lhs)?.to_string();
    let detail = format!("function({})", arguments.join(", "));

    // build the document symbol
    let symbol = DocumentSymbol {
        name,
        kind: SymbolKind::FUNCTION,
        detail: Some(detail),
        children: Some(Vec::new()),
        deprecated: None,
        tags: None,
        range: Range {
            start: convert_point_to_position(contents, lhs.start_position()),
            end: convert_point_to_position(contents, rhs.end_position()),
        },
        selection_range: Range {
            start: convert_point_to_position(contents, lhs.start_position()),
            end: convert_point_to_position(contents, lhs.end_position()),
        },
    };

    // add this symbol to the parent node
    parent.children.as_mut().unwrap().push(symbol);

    // recurse into this node
    let parent = parent.children.as_mut().unwrap().last_mut().unwrap();
    index_node(&rhs, contents, parent, symbols)?;

    Ok(true)
}
