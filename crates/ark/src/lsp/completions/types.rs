//
// types.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::path::PathBuf;

use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;
use tower_lsp::lsp_types::TextDocumentPositionParams;
use tree_sitter::Node;
use tree_sitter::Point;

use crate::lsp::documents::Document;
use crate::lsp::traits::position::PositionExt;
use crate::lsp::traits::tree::TreeExt;

// TODO: This is also used in `hover()`, so should probably just be
// a more generic "DocumentContext" type
#[derive(Debug)]
pub struct CompletionContext<'a> {
    pub document: &'a Document,
    pub node: Node<'a>,
    pub source: String,
    pub point: Point,
}

pub fn completion_context<'a>(
    document: &'a Document,
    position: &TextDocumentPositionParams,
) -> Result<CompletionContext<'a>> {
    // get reference to AST
    let ast = &document.ast;

    // try to find node at completion position
    let point = position.position.as_point();

    // use the node to figure out the completion token
    let node = ast.node_at_point(point);
    let source = document.contents.to_string();

    // build completion context
    Ok(CompletionContext {
        document,
        node,
        source,
        point,
    })
}

#[derive(Serialize, Deserialize, Debug)]
pub enum CompletionData {
    DataVariable {
        name: String,
        owner: String,
    },
    Directory {
        path: PathBuf,
    },
    File {
        path: PathBuf,
    },
    Function {
        name: String,
        package: Option<String>,
    },
    Object {
        name: String,
    },
    Package {
        name: String,
    },
    Parameter {
        name: String,
        function: String,
    },
    RoxygenTag {
        tag: String,
    },
    ScopeParameter {
        name: String,
    },
    ScopeVariable {
        name: String,
    },
    Snippet {
        text: String,
    },
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PromiseStrategy {
    Simple,
    Force,
}
