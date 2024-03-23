/*
 * line_ending.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

pub use sys::line_ending::NATIVE_LINE_ENDING;

use crate::sys;

pub const POSIX_LINE_ENDING: &'static str = "\n";
pub const WINDOWS_LINE_ENDING: &'static str = "\r\n";

#[derive(Debug)]
pub enum LineEnding {
    Windows,
    Posix,
    Native,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Windows => WINDOWS_LINE_ENDING,
            LineEnding::Posix => POSIX_LINE_ENDING,
            LineEnding::Native => NATIVE_LINE_ENDING,
        }
    }
}

pub fn convert_line_endings(s: &str, eol_type: LineEnding) -> String {
    // so far, no demonstrated need to repair anything other than CRLF, hence
    // the `from` value
    s.replace("\r\n", eol_type.as_str())
}

#[test]
fn test_convert_line_endings_explicit() {
    // [\r] [\n]
    let s = "\r\n";

    let posix = convert_line_endings(s, LineEnding::Posix);
    assert_eq!(posix, "\n");

    let windows = convert_line_endings(s, LineEnding::Windows);
    assert_eq!(windows, s);

    // [a] [\] [r] [\] [n] [b]
    let s2 = r#"a\r\nb"#;
    let s2_res = convert_line_endings(s2, LineEnding::Posix);
    assert_eq!(s2_res, s2);
}
