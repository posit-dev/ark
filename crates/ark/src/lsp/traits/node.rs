// node.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use anyhow::anyhow;
use stdext::all;
use stdext::result::ResultExt;
use tree_sitter::Node;
use tree_sitter::Point;
use tree_sitter::Range;
use tree_sitter::TreeCursor;

use crate::lsp::traits::point::PointExt;

fn _dump_impl(cursor: &mut TreeCursor, source: &str, indent: &str, output: &mut String) {
    let node = cursor.node();

    if node.start_position().row == node.end_position().row {
        // write line
        output.push_str(
            format!(
                "{} - {} - {} ({} -- {})\n",
                indent,
                node.node_as_str(&source).unwrap(),
                node.kind(),
                node.start_position(),
                node.end_position(),
            )
            .as_str(),
        );
    }

    if cursor.goto_first_child() {
        let indent = format!("  {}", indent);
        _dump_impl(cursor, source, indent.as_str(), output);
        while cursor.goto_next_sibling() {
            _dump_impl(cursor, source, indent.as_str(), output);
        }

        cursor.goto_parent();
    }
}

pub struct FwdLeafIterator<'a> {
    pub node: Node<'a>,
}

impl<'a> Iterator for FwdLeafIterator<'a> {
    type Item = Node<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(node) = self.node.next_leaf() {
            self.node = node;
            Some(node)
        } else {
            None
        }
    }
}

pub struct BwdLeafIterator<'a> {
    pub node: Node<'a>,
}

impl<'a> Iterator for BwdLeafIterator<'a> {
    type Item = Node<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(node) = self.node.prev_leaf() {
            self.node = node;
            Some(node)
        } else {
            None
        }
    }
}

pub trait NodeExt: Sized {
    fn dump(&self, source: &str) -> String;

    fn find_parent(&self, callback: impl Fn(&Self) -> bool) -> Option<Self>;

    fn find_smallest_spanning_node(&self, point: Point) -> Option<Self>;
    fn find_closest_node_to_point(&self, point: Point) -> Option<Self>;

    fn prev_leaf(&self) -> Option<Self>;
    fn next_leaf(&self) -> Option<Self>;

    fn fwd_leaf_iter(&self) -> FwdLeafIterator<'_>;
    fn bwd_leaf_iter(&self) -> BwdLeafIterator<'_>;

    fn ancestors(&self) -> impl Iterator<Item = Self>;
    fn children_of(node: Self) -> impl Iterator<Item = Self>;
    fn next_siblings(&self) -> impl Iterator<Item = Self>;
    fn arguments(&self) -> impl Iterator<Item = (Option<Self>, Option<Self>)>;
    fn arguments_values(&self) -> impl Iterator<Item = Option<Self>>;
    fn arguments_names(&self) -> impl Iterator<Item = Option<Self>>;
    fn arguments_names_as_string(&self, contents: &str) -> impl Iterator<Item = Option<String>>;

    /// Return the node's text as a `&str` slice into `source`.
    /// This is a thin wrapper around `Node::utf8_text(&node, source.as_bytes())`.
    fn node_as_str<'a>(&self, source: &'a str) -> anyhow::Result<&'a str>;

    /// Convenience method returning an owned `String` for this node's text.
    fn node_to_string(&self, source: &str) -> anyhow::Result<String>;
}

impl<'tree> NodeExt for Node<'tree> {
    fn dump(&self, source: &str) -> String {
        let mut output = "\n".to_string();
        _dump_impl(&mut self.walk(), source, "", &mut output);
        return output;
    }

    fn find_parent(&self, callback: impl Fn(&Self) -> bool) -> Option<Self> {
        let mut node = *self;
        loop {
            if callback(&node) {
                return Some(node);
            }

            node = match node.parent() {
                Some(node) => node,
                None => return None,
            }
        }
    }

    fn find_smallest_spanning_node(&self, point: Point) -> Option<Self> {
        // The only way this should ever be `None` is if `Point` is not in the AST span
        _find_smallest_container(&self, point)
    }

    fn find_closest_node_to_point(&self, point: Point) -> Option<Self> {
        match _find_smallest_container(&self, point) {
            Some(node) => _find_closest_child(&node, point),
            None => None,
        }
    }

    fn prev_leaf(&self) -> Option<Self> {
        // Walk up the tree, until we find a node with a previous sibling.
        // Then, move to that sibling.
        // Finally, descend down the last children of that node, if any.
        //
        //    x _ _ < _ _ x
        //    |           |
        //    v           ^
        //    |           |
        //    x           x
        //
        let mut node = *self;
        while node.prev_sibling().is_none() {
            node = match node.parent() {
                Some(parent) => parent,
                None => return None,
            }
        }

        node = node.prev_sibling().unwrap();

        loop {
            let count = node.child_count();
            if count == 0 {
                break;
            }

            node = node.child(count - 1).unwrap();
            continue;
        }

        Some(node)
    }

    fn next_leaf(&self) -> Option<Self> {
        // Walk up the tree, until we find a node with a sibling.
        // Then, move to that sibling.
        // Finally, descend down the first children of that node, if any.
        //
        //    x _ _ > _ _ x
        //    |           |
        //    ^           v
        //    |           |
        //    x           x
        //
        let mut node = *self;
        while node.next_sibling().is_none() {
            node = match node.parent() {
                Some(parent) => parent,
                None => return None,
            }
        }

        node = node.next_sibling().unwrap();

        loop {
            if let Some(child) = node.child(0) {
                node = child;
                continue;
            }
            break;
        }

        Some(node)
    }

    fn fwd_leaf_iter(&self) -> FwdLeafIterator<'_> {
        FwdLeafIterator { node: *self }
    }

    fn bwd_leaf_iter(&self) -> BwdLeafIterator<'_> {
        BwdLeafIterator { node: *self }
    }

    fn ancestors(&self) -> impl Iterator<Item = Node<'tree>> {
        // We'd ideally use the cursor API here too but
        // `ts_tree_cursor_goto_parent()` doesn't behave like
        // `ts_node_parent()`: the latter traverses `ERROR` nodes but not the
        // former. So for now we accept the performance hit of tree traversal at
        // each `parent()` call.
        std::iter::successors(Some(*self), |p| p.parent())
    }

    fn next_siblings(&self) -> impl Iterator<Item = Node<'tree>> {
        let mut cursor = self.walk();

        std::iter::from_fn(move || {
            if cursor.goto_next_sibling() {
                Some(cursor.node())
            } else {
                None
            }
        })
    }

    fn children_of(node: Node<'tree>) -> impl Iterator<Item = Node<'tree>> {
        let mut cursor = node.walk();
        let mut done = !cursor.goto_first_child();

        std::iter::from_fn(move || {
            if done {
                None
            } else {
                let item = Some(cursor.node());
                done = !cursor.goto_next_sibling();
                item
            }
        })
    }

    /// Iterator over argument names and values. Either of `name` and `value`
    /// may be absent, but not both.
    fn arguments(&self) -> impl Iterator<Item = (Option<Node<'tree>>, Option<Node<'tree>>)> {
        self.child_by_field_name("arguments")
            // Create iterator that unpacks Option with `flat_map()`
            .into_iter()
            .flat_map(Self::children_of)
            .filter_map(|node| {
                if node.kind() != "argument" {
                    return None;
                }

                let name = node.child_by_field_name("name");
                let value = node.child_by_field_name("value");

                Some((name, value))
            })
    }

    fn arguments_names(&self) -> impl Iterator<Item = Option<Node<'tree>>> {
        self.arguments().map(|(name, _value)| name)
    }

    fn arguments_names_as_string(&self, contents: &str) -> impl Iterator<Item = Option<String>> {
        // Note: capture `contents` by reference into the closure so the iterator
        // can outlive the stack frame where this method is called.
        self.arguments_names().map(move |maybe_node| {
            maybe_node.and_then(|node| match node.node_as_str(&contents) {
                Err(err) => {
                    tracing::error!("Can't convert argument name to text: {err:?}");
                    None
                },
                Ok(text) => Some(text.to_string()),
            })
        })
    }

    fn node_as_str<'a>(&self, source: &'a str) -> anyhow::Result<&'a str> {
        self.utf8_text(source.as_bytes()).anyhow()
    }

    fn node_to_string(&self, source: &str) -> anyhow::Result<String> {
        Ok(self
            .node_as_str(source)
            .map(|s| s.to_string())
            .map_err(|e| anyhow!(e))?)
    }

    fn arguments_values(&self) -> impl Iterator<Item = Option<Node<'tree>>> {
        self.arguments().map(|(_name, value)| value)
    }
}

/// First, recurse through children to find the smallest
/// node that contains the requested point.
fn _find_smallest_container<'a>(node: &Node<'a>, point: Point) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let children = node.children(&mut cursor);

    for child in children {
        if _range_contains_point(child.range(), point) {
            return _find_smallest_container(&child, point);
        }
    }

    // No child contained the `point`, revert back to parent
    if _range_contains_point(node.range(), point) {
        Some(*node)
    } else {
        None
    }
}

// For "containment", here we use `[]`. Ambiguities between `]` and `[` of
// adjacent nodes are solved by taking the first child that "contains" the point.
fn _range_contains_point(range: Range, point: Point) -> bool {
    all!(
        range.start_point.is_before_or_equal(point),
        range.end_point.is_after_or_equal(point)
    )
}

/// Next, recurse through the children of this node
/// (if any) to find the closest child.
fn _find_closest_child<'a>(node: &Node<'a>, point: Point) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let children = node.children(&mut cursor);

    // Node iterators don't implement `rev()`, presumably for performance, but
    // this is the cleanest way to implement this so we collect into a vector
    // first.
    let children: Vec<Node> = children.collect();

    // Loop backwards through children. First time the `start` is before the
    // `point` corresponds to the last child this is `true` for, which we then
    // recurse into.
    for child in children.into_iter().rev() {
        if child.range().start_point.is_before_or_equal(point) {
            return _find_closest_child(&child, point);
        }
    }

    // No children start before the `point`, revert back to parent
    // (probably rare)
    if node.range().start_point.is_before_or_equal(point) {
        Some(*node)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use tree_sitter::Parser;
    use tree_sitter::Point;

    use crate::fixtures::point_from_cursor;
    use crate::lsp::traits::node::NodeExt;

    #[test]
    #[rustfmt::skip]
    fn test_point_in_whitespace() {
        let text = "
fn <- function(x, arg) {
  if (is.null(arg)) {
  @  return(x)
  }
}
";

        let (text, point) = point_from_cursor(text);

        // create a parser for this document
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_r::LANGUAGE.into())
            .expect("failed to create parser");

        let tree = parser.parse(text, None).unwrap();
        let node = tree.root_node();

        let node = node.find_closest_node_to_point(point).unwrap();

        // It takes into account anonymous nodes, so the lone `{` is the closest node
        // that is still before the `@` cursor position.
        // Note that if it is important that the selected node "contains" the point, then
        // this is the wrong thing to use. If we just want the absolute closest node where
        // the only requirement is that it starts before the point, then this is correct.
        assert_eq!(node.start_position(), Point::new(2, 20));
        assert_eq!(node.end_position(), Point::new(2, 21))
    }
}
