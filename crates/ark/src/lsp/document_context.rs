//
// document_context.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use tree_sitter::Node;
use tree_sitter::Point;

use crate::lsp::documents::Document;
use crate::lsp::traits::node::NodeExt;

#[derive(Debug)]
pub struct DocumentContext<'a> {
    pub document: &'a Document,
    pub node: Node<'a>,
    pub source: String,
    pub point: Point,
    pub trigger: Option<String>,
}

impl<'a> DocumentContext<'a> {
    pub fn new(document: &'a Document, point: Point, trigger: Option<String>) -> Self {
        // get reference to AST
        let ast = &document.ast;

        // find node at point
        let node = ast.root_node().find_closest_node_to_point(point).unwrap();

        // convert document contents to a string once, to be reused elsewhere
        let source = document.contents.to_string();

        // build document context
        DocumentContext {
            document,
            node,
            source,
            point,
            trigger,
        }
    }
}

#[test]
fn test_document_context_start_of_document() {
    let point = Point { row: 0, column: 0 };

    // Empty document
    let document = Document::new("");
    let context = DocumentContext::new(&document, point, None);
    assert_eq!(
        context.node.utf8_text(context.source.as_bytes()).unwrap(),
        ""
    );

    // Start of document with text
    let document = Document::new("1 + 1");
    let context = DocumentContext::new(&document, point, None);
    assert_eq!(
        context.node.utf8_text(context.source.as_bytes()).unwrap(),
        "1"
    );
}
