/*
 * console.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use std::ffi::c_char;
use std::ffi::CStr;

use once_cell::sync::Lazy;
use regex::bytes::Regex;

use super::strings::code_page_to_utf8;
use super::strings::get_system_code_page;

// - (?-u) to disable unicode so it matches the bytes exactly
// - (?s:.) so `.` matches anything INCLUDING new lines
// https://github.com/rust-lang/regex/blob/837fd85e79fac2a4ea64030411b9a4a7b17dfa42/src/builders.rs#L368-L372
static RE_EMBEDDED_UTF8: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?-u)\x02\xFF\xFE(?<text>(?s:.)*?)\x03\xFF\xFE").unwrap());

/// NOTE: On Windows with GUIs, when R attempts to write text to
/// the console, it will surround UTF-8 text with 3-byte escapes:
///
///    \002\377\376 <text> \003\377\376
///
/// strangely, we see these escapes around text that is not UTF-8
/// encoded but rather is encoded according to the active locale.
/// extract those pieces of text (discarding the escapes) and
/// convert to UTF-8. (still not exactly sure what the cause of this
/// behavior is; perhaps there is an extra UTF-8 <-> system conversion
/// happening somewhere in the pipeline?)
pub fn console_to_utf8(x: *const c_char) -> anyhow::Result<String> {
    let code_page = get_system_code_page();

    let x = unsafe { CStr::from_ptr(x) };

    // Drops trailing nul terminator
    let mut x = x.to_bytes();

    let mut out = Vec::new();

    while let Some(capture) = RE_EMBEDDED_UTF8.captures(x) {
        // `get(0)` always returns the full match
        let full = capture.get(0).unwrap();

        if full.start() > 0 {
            // Translate everything up to right before the match
            // and add to the output
            let slice = code_page_to_utf8(&x[..full.start()], code_page)?;
            out.push(slice);
        }

        // Add everything in the `text` capture group.
        // By definition, this is already UTF-8.
        let text = capture.name("text").unwrap().as_bytes();
        let text = std::str::from_utf8(text).unwrap();
        let text = text.to_string();
        out.push(text);

        // Advance `x`
        x = &x[full.end()..];
    }

    if x.len() > 0 {
        // Translate everything that's left and add to the output
        let slice = code_page_to_utf8(x, code_page)?;
        out.push(slice);
    }

    let out = out.join("");

    Ok(out)
}
