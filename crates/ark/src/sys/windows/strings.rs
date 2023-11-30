/*
 * strings.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use winsafe::co::CP;
use winsafe::co::MBC;
use winsafe::MultiByteToWideChar;
use winsafe::WideCharToMultiByte;

/// Convert a string encoded in the system `code_page` to UTF-8
///
/// Only useful on Windows, on other systems we are always in UTF-8.
pub fn system_to_utf8(x: &[u8], code_page: CP) -> anyhow::Result<String> {
    let flags = MBC::NoValue;

    let x = MultiByteToWideChar(code_page, flags, x)?;

    let default_char = None;
    let used_default_char = None;

    let x = WideCharToMultiByte(CP::UTF8, flags, &x, default_char, used_default_char)?;

    let x = String::from_utf8(x)?;

    Ok(x)
}
