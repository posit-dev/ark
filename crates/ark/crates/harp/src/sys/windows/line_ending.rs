/*
 * line_ending.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use crate::line_ending::WINDOWS_LINE_ENDING;

pub const NATIVE_LINE_ENDING: &'static str = WINDOWS_LINE_ENDING;

#[test]
fn test_convert_line_endings_native_windows() {
    use crate::line_ending::convert_line_endings;
    use crate::line_ending::LineEnding;

    let res = convert_line_endings("\r\n", LineEnding::Native);
    assert_eq!(res, "\r\n");
}
