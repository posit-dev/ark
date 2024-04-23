//
// selection_range.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use tree_sitter::Node;
use tree_sitter::Point;
use tree_sitter::Range;
use tree_sitter::Tree;

use crate::lsp::encoding::convert_tree_sitter_range_to_lsp_range;

/// A mirror of `tower_lsp::lsp_types::SelectionRange`, but using tree-sitter ranges
pub struct SelectionRange {
    pub range: Range,
    pub parent: Option<Box<SelectionRange>>,
}

pub fn selection_range(tree: &Tree, points: Vec<Point>) -> Option<Vec<SelectionRange>> {
    // If there is a `None` element encountered, the `collect()` promotes the individual
    // element `None` into a `None` for the entire result, which we do want, as otherwise
    // we could end up with a partially invalid multi-selection, which is worse than
    // doing nothing.
    points
        .into_iter()
        .map(|point| selection_range_one(tree, point))
        .collect()
}

fn selection_range_one(tree: &Tree, point: Point) -> Option<SelectionRange> {
    // Checks only named nodes to find the smallest named node that contains
    // the point using the following definition of containment:
    // - `node.start_position() <= start`
    // - `node.end_position() > start`
    // - `node.end_position() >= end`
    // which reduces to this when you consider that for us `start == end == point`
    // - `node.start_position() <= point`
    // - `node.end_position() > point`
    // So, for example, `{ 1 + 1 }@` won't select the braces (we are past them) but
    // `@{ 1 + 1 }` will (we are about to enter them).
    let Some(node) = tree
        .root_node()
        .named_descendant_for_point_range(point, point)
    else {
        log::error!("Failed to find containing node for point: {point}.");
        return None;
    };

    Some(selection_range_build(node))
}

fn selection_range_build(node: Node) -> SelectionRange {
    let range = node.range();

    let parent = node.parent().and_then(|parent| {
        let selection = selection_range_build(parent);
        Some(Box::new(selection))
    });

    SelectionRange { range, parent }
}

pub fn convert_selection_range_from_tree_sitter_to_lsp(
    selection: SelectionRange,
    document: &crate::lsp::documents::Document,
) -> tower_lsp::lsp_types::SelectionRange {
    let range = convert_tree_sitter_range_to_lsp_range(&document.contents, selection.range);

    // If there is a parent, convert it and box it
    let parent = selection.parent.and_then(|selection| {
        let selection = convert_selection_range_from_tree_sitter_to_lsp(*selection, document);
        Some(Box::new(selection))
    });

    tower_lsp::lsp_types::SelectionRange { range, parent }
}

#[cfg(test)]
mod tests {
    use tree_sitter::Parser;
    use tree_sitter::Point;

    use crate::lsp::selection_range::selection_range;
    use crate::test::point_from_cursor;

    #[test]
    #[rustfmt::skip]
    fn test_before_braces() {
        let text = "
@{
  1 + 1
}

2
";

        let (text, point) = point_from_cursor(text);

        let language = tree_sitter_r::language();

        let mut parser = Parser::new();
        parser
            .set_language(&language)
            .expect("failed to create parser");

        let tree = parser.parse(text, None).unwrap();

        let points = Vec::from([point]);

        let selections = selection_range(&tree, points).unwrap();

        // Two selections, the braces and the whole document
        let selection = selections.get(0).unwrap();
        assert_eq!(selection.range.start_point, Point::new(1, 0));
        assert_eq!(selection.range.end_point, Point::new(3, 1));

        let selection = selection.parent.as_ref().unwrap();
        assert_eq!(selection.range.start_point, Point::new(1, 0));
        assert_eq!(selection.range.end_point, Point::new(6, 0));
        assert!(selection.parent.is_none());
    }

    #[test]
    #[rustfmt::skip]
    fn test_after_braces() {
        let text = "
{
  1 + 1
}@

2
";

        let (text, point) = point_from_cursor(text);

        let language = tree_sitter_r::language();

        let mut parser = Parser::new();
        parser
            .set_language(&language)
            .expect("failed to create parser");

        let tree = parser.parse(text, None).unwrap();

        let points = Vec::from([point]);

        let selections = selection_range(&tree, points).unwrap();

        // Just 1 selection, the whole document
        let selection = selections.get(0).unwrap();
        assert_eq!(selection.range.start_point, Point::new(1, 0));
        assert_eq!(selection.range.end_point, Point::new(6, 0));
        assert!(selection.parent.is_none());
    }

    #[test]
    #[rustfmt::skip]
    fn test_selection_range_recursiveness() {
        let text = "
fn <- function(x, arg) {
  if (is.null(arg)) {
  @  return(x)
  }
}
";

        let (text, point) = point_from_cursor(text);

        let language = tree_sitter_r::language();

        // create a parser for this document
        let mut parser = Parser::new();
        parser
            .set_language(&language)
            .expect("failed to create parser");

        let tree = parser.parse(text, None).unwrap();

        let points = Vec::from([point]);

        let selections = selection_range(&tree, points).unwrap();

        // Braces for if statement
        let selection = selections.get(0).unwrap();
        assert_eq!(selection.range.start_point, Point::new(2, 20));
        assert_eq!(selection.range.end_point, Point::new(4, 3));

        // If statement itself
        let selection = selection.parent.as_ref().unwrap();
        assert_eq!(selection.range.start_point, Point::new(2, 2));
        assert_eq!(selection.range.end_point, Point::new(4, 3));

        // Braces for function
        let selection = selection.parent.as_ref().unwrap();
        assert_eq!(selection.range.start_point, Point::new(1, 23));
        assert_eq!(selection.range.end_point, Point::new(5, 1));

        // Function itself
        let selection = selection.parent.as_ref().unwrap();
        assert_eq!(selection.range.start_point, Point::new(1, 6));
        assert_eq!(selection.range.end_point, Point::new(5, 1));

        // `<-` operator
        let selection = selection.parent.as_ref().unwrap();
        assert_eq!(selection.range.start_point, Point::new(1, 0));
        assert_eq!(selection.range.end_point, Point::new(5, 1));

        // Whole document
        let selection = selection.parent.as_ref().unwrap();
        assert_eq!(selection.range.start_point, Point::new(1, 0));
        assert_eq!(selection.range.end_point, Point::new(6, 0));
        assert!(selection.parent.is_none());
    }
}
