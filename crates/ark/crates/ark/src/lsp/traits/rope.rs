//
// rope.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use ropey::Rope;
use ropey::RopeSlice;
use tree_sitter::Node;
use tree_sitter::Point;

pub trait RopeExt<'a> {
    fn point_to_byte(&self, point: Point) -> usize;
    fn node_slice(&'a self, node: &Node) -> std::result::Result<RopeSlice<'a>, anyhow::Error>;
}

impl<'a> RopeExt<'a> for Rope {
    fn point_to_byte(&self, point: Point) -> usize {
        self.line_to_byte(point.row) + point.column
    }

    fn node_slice(&'a self, node: &Node) -> std::result::Result<RopeSlice<'a>, anyhow::Error> {
        // For some reason Ropey returns an Option and hides the Result which includes
        // the actual Error reason. We convert `None` back to an error so we can propagate it.
        let range = node.start_byte()..node.end_byte();

        if let Some(slice) = self.get_byte_slice(range) {
            return Ok(slice);
        }

        let message = anyhow::anyhow!(
            "Failed to slice Rope at byte range {}-{}. Text: '{}'.",
            node.start_byte(),
            node.end_byte(),
            self.to_string()
        );

        Err(message)
    }
}
