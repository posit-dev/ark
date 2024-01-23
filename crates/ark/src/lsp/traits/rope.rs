//
// rope.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use ropey::Rope;
use ropey::RopeSlice;
use tower_lsp::lsp_types::Position;
use tree_sitter::Node;

pub trait RopeExt<'a> {
    fn position_to_byte(&self, position: Position) -> usize;
    fn node_slice(&'a self, node: &Node) -> std::result::Result<RopeSlice<'a>, anyhow::Error>;
}

impl<'a> RopeExt<'a> for Rope {
    fn position_to_byte(&self, position: Position) -> usize {
        self.line_to_byte(position.line as usize) + position.character as usize
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
