//
// range.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use stdext::all;
use tree_sitter::Point;
use tree_sitter::Range;

use crate::lsp::traits::point::PointExt;

pub trait RangeExt {
    // Left open range, like `(]`, to ensure that with `(|)` where the
    // cursor is at `|`, the `(` node owns the point, not the `)`, with no
    // ambiguity.
    fn contains_point(&self, point: Point) -> bool;
}

impl RangeExt for Range {
    fn contains_point(&self, point: Point) -> bool {
        all!(
            self.start_point.is_before(point),
            self.end_point.is_after_or_equal(point)
        )
    }
}
