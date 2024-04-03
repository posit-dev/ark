//
// test.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

// Wrapper around `harp::r_test_impl()` that also initializes the ark level R
// modules, so they can be utilized in the tests

use std::sync::Once;

use tree_sitter::Point;

use crate::modules;

pub fn r_test<F: FnOnce()>(f: F) {
    let f = || {
        initialize_ark();
        f()
    };
    harp::test::r_test(f)
}

static INIT: Once = Once::new();

fn initialize_ark() {
    INIT.call_once(|| {
        // Initialize the positron module so tests can use them.
        // Routines are already registered by `harp::test::r_test()`.
        modules::initialize(true).unwrap();
    });
}

pub fn point_from_cursor(x: &str) -> (String, Point) {
    let lines = x.split("\n").collect::<Vec<&str>>();

    // i.e. looking for `@` in something like `fn(x = @1, y = 2)`, and it treats the
    // `@` as the cursor position
    let cursor = b'@';

    for (line_row, line) in lines.into_iter().enumerate() {
        for (char_column, char) in line.as_bytes().into_iter().enumerate() {
            if char == &cursor {
                let x = x.replace("@", "");
                let point = Point {
                    row: line_row,
                    column: char_column,
                };
                return (x, point);
            }
        }
    }

    panic!("`x` must include a `@` character!");
}

#[cfg(test)]
mod tests {
    use tree_sitter::Point;

    use crate::test::point_from_cursor;

    #[test]
    #[rustfmt::skip]
    fn test_point_from_cursor() {
        let (text, point) = point_from_cursor("1@ + 2");
        assert_eq!(text, "1 + 2".to_string());
        assert_eq!(point, Point::new(0, 1));

        let text =
"fn(
  arg =@ 3
)";
        let expect =
"fn(
  arg = 3
)";
        let (text, point) = point_from_cursor(text);
        assert_eq!(text, expect);
        assert_eq!(point, Point::new(1, 7));
    }
}
