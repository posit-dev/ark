/*
 * console.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use std::ffi::c_char;
use std::ffi::CStr;

/// On Unix, we assume that the buffer to write to the console is
/// already in UTF-8
pub fn console_to_utf8(x: *const c_char) -> anyhow::Result<String> {
    let x = unsafe { CStr::from_ptr(x) };

    let x = match x.to_str() {
        Ok(content) => content,
        Err(err) => panic!("Failed to read from R buffer: {err:?}"),
    };

    Ok(x.to_string())
}
