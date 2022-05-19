// 
// rope.rs
// 
// Copyright (C) 2022 by RStudio, PBC
// 
// 

use ropey::Rope;
use tower_lsp::lsp_types::Position;
use tree_sitter::Point;

pub(crate) trait RopeExt {
    fn point_to_byte(&self, point: Point) -> usize;
    fn position_to_byte(&self, position: Position) -> usize;
}

impl RopeExt for Rope {

    fn point_to_byte(&self, point: Point) -> usize {
        self.line_to_byte(point.row) + point.column
    }

    fn position_to_byte(&self, position: Position) -> usize {
        self.line_to_byte(position.line as usize) + position.character as usize
    }

}
