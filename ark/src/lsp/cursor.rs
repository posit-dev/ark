// 
// cursor.rs
// 
// Copyright (C) 2022 by RStudio, PBC
// 
// 

use tree_sitter::{Node, Point, TreeCursor};

use crate::lsp::{point::PointExt, logger::LOGGER};

// Extension trait for the TreeSitter cursor object.
pub(crate) trait TreeCursorExt {

    // Recurse through all nodes in an AST, invoking a callback as those nodes
    // are visited. The callback should return 'false' if recursion needs to be
    // stopped early.
    fn recurse<Callback: FnMut(Node) -> bool>(&mut self, callback: Callback);

    // Internal method used for recursion.
    fn _recurse_impl<Callback: FnMut(Node) -> bool>(&mut self, callback: &mut Callback);

    // Find the node closest to the requested point (if any). The node closest
    // to this point will be used.
    fn go_to_point(&mut self, point: Point);

    // Move the cursor to the parent node satisfying some callback condition.
    fn find_parent<Callback: FnMut(Node) -> bool>(&mut self, callback: Callback) -> bool;
    
}

impl TreeCursorExt for TreeCursor<'_> {

    fn recurse<Callback: FnMut(Node) -> bool>(&mut self, mut callback: Callback) {
        self._recurse_impl(&mut callback);
    }

    fn _recurse_impl<Callback: FnMut(Node) -> bool>(&mut self, callback: &mut Callback) {

        let recurse = callback(self.node());

        if recurse && self.goto_first_child() {

            self._recurse_impl(callback);
            while self.goto_next_sibling() {
                self._recurse_impl(callback);
            }

            self.goto_parent();

        }

    }

    fn go_to_point(&mut self, point: Point) {

        // debugging; remove later
        unsafe { LOGGER.append(format!("Looking for point: {:?}", point).as_str()) };

        self.recurse(|node| {
            
            // debugging; remove later
            let message = format!("{:?}", node);
            unsafe { LOGGER.append(message.as_str()) };

            if node.start_position().is_before_or_equal(&point) {
                unsafe { LOGGER.append("Position is before; recursing.") };
                return true;
            } else {
                unsafe { LOGGER.append("Position is not before; not recursing.") };
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
