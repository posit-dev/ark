//
// fixtures/utils.rs
//
// Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
//
//

use std::sync::Once;

use tree_sitter::Point;

use crate::modules;
use crate::modules::ARK_ENVS;

static INIT: Once = Once::new();

pub fn r_test_init() {
    harp::fixtures::r_test_init();
    INIT.call_once(|| {
        // Initialize the positron module so tests can use them.
        modules::initialize().unwrap();
    });
}

pub fn point_from_cursor(x: &str) -> (String, Point) {
    // i.e. looking for `@` in something like `fn(x = @1, y = 2)`, and it treats the
    // `@` as the cursor position
    let (text, point, _offset) = point_and_offset_from_cursor(x, b'@');
    (text, point)
}

/// Looks for `cursor` in the text and interprets it as the user's cursor position
pub fn point_and_offset_from_cursor(x: &str, cursor: u8) -> (String, Point, usize) {
    let lines = x.split("\n").collect::<Vec<&str>>();

    let mut offset = 0;

    let cursor_for_replace = [cursor];
    let cursor_for_replace = str::from_utf8(&cursor_for_replace).unwrap();

    for (line_row, line) in lines.into_iter().enumerate() {
        for (char_column, char) in line.as_bytes().into_iter().enumerate() {
            if char == &cursor {
                let x = x.replace(cursor_for_replace, "");
                let point = Point {
                    row: line_row,
                    column: char_column,
                };
                offset += char_column;
                return (x, point, offset);
            }
        }
        // `+ 1` for the removed `\n` at the end of this line
        offset += line.as_bytes().len() + 1;
    }

    panic!("`x` must include a `@` character!");
}

pub fn package_is_installed(package: &str) -> bool {
    harp::parse_eval0(
        format!(".ps.is_installed('{package}')").as_str(),
        ARK_ENVS.positron_ns,
    )
    .unwrap()
    .try_into()
    .unwrap()
}

#[cfg(test)]
mod tests {
    use tree_sitter::Point;

    use crate::fixtures::point_and_offset_from_cursor;

    #[test]
    #[rustfmt::skip]
    fn test_point_and_offset_from_cursor() {
        let (text, point, offset) = point_and_offset_from_cursor("1@ + 2", b'@');
        assert_eq!(text, "1 + 2".to_string());
        assert_eq!(point, Point::new(0, 1));
        assert_eq!(offset, 1);

        let text =
"fn(
  arg =@ 3
)";
        let expect =
"fn(
  arg = 3
)";
        let (text, point, offset) = point_and_offset_from_cursor(text, b'@');
        assert_eq!(text, expect);
        assert_eq!(point, Point::new(1, 7));
        assert_eq!(offset, 11);
    }
}
