// 
// cursor.rs
// 
// Copyright (C) 2022 by RStudio, PBC
// 
// 

use tree_sitter::{Node, Point, TreeCursor};

use crate::lsp::{point::PointExt};

// Extension trait for the TreeSitter cursor object.
pub(crate) trait TreeCursorExt {

    // Recurse through all nodes in an AST, invoking a callback as those nodes
    // are visited. The callback should return 'false' if recursion needs to be
    // stopped early.
    fn recurse<Callback: FnMut(Node) -> bool>(&mut self, callback: Callback);

    // Internal method used for recursion.
    fn _recurse_impl<Callback: FnMut(Node) -> bool>(&mut self, callback: &mut Callback) -> bool;

    // Find the node closest to the requested point (if any). The node closest
    // to this point will be used.
    fn goto_point(&mut self, point: Point);

    // Move the cursor to the parent node satisfying some callback condition.
    fn find_parent<Callback: FnMut(Node) -> bool>(&mut self, callback: Callback) -> bool;
    
}

impl TreeCursorExt for TreeCursor<'_> {

    fn recurse<Callback: FnMut(Node) -> bool>(&mut self, mut callback: Callback) {
        self._recurse_impl(&mut callback);
    }

    fn _recurse_impl<Callback: FnMut(Node) -> bool>(&mut self, callback: &mut Callback) -> bool {

        if !callback(self.node()) {
            return false;
        }

        if self.goto_first_child() {

            if !self._recurse_impl(callback) {
                return false;
            }

            while self.goto_next_sibling() {

                if !self._recurse_impl(callback) {
                    return false;
                }

            }

            self.goto_parent();

        }

        return true;

    }

    fn goto_point(&mut self, point: Point) {

        // TODO: logic here is not quite right
        self.recurse(|node| {
            if node.start_position().is_before_or_equal(&point) {
                return true;
            } else {
                return false;
            }
        });

    }

    fn find_parent<Callback: FnMut(Node) -> bool>(&mut self, mut callback: Callback) -> bool {

        while self.goto_parent() {
            if callback(self.node()) {
                return true;
            }
        }

        return false;

    }


}
