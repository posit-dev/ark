/*
 * strings.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use winsafe::co::CP;
use winsafe::co::MBC;
use winsafe::co::WC;
use winsafe::MultiByteToWideChar;
use winsafe::WideCharToMultiByte;

pub fn system_to_utf8(x: &[u8]) -> anyhow::Result<String> {
    let code_page = get_system_code_page();
    code_page_to_utf8(x, code_page)
}

/// Convert a string encoded in the `code_page` to UTF-8
///
/// Only useful on Windows, on other systems we are always in UTF-8.
pub fn code_page_to_utf8(x: &[u8], code_page: CP) -> anyhow::Result<String> {
    // According to the below link, `dwFlags`, which corresponds to this, must
    // be set to `0` for some code pages, which corresponds to `NoValue`.
    // https://learn.microsoft.com/en-us/windows/win32/api/stringapiset/nf-stringapiset-multibytetowidechar#parameters
    let flags = MBC::NoValue;

    let x = MultiByteToWideChar(code_page, flags, x)?;

    // TODO: Windows
    // winsafe currently adds an extra byte for safety, but that makes the strings too long
    // and in our experience it has never been necessary. Remove this if it gets fixed.
    // https://github.com/rodrigocfd/winsafe/issues/111
    let size = x.len() - 1;
    let x = &x[..size];

    // `WC::NoValue` doesn't exist, so we make it unsafely:
    // https://github.com/rodrigocfd/winsafe/issues/110
    let flags = unsafe { WC::from_raw(0) };
    let default_char = None;
    let used_default_char = None;

    let x = WideCharToMultiByte(CP::UTF8, flags, x, default_char, used_default_char)?;

    // TODO: Windows
    // winsafe currently adds an extra byte for safety, but that makes the strings too long
    // and in our experience it has never been necessary. Remove this if it gets fixed.
    // https://github.com/rodrigocfd/winsafe/issues/111
    let size = x.len() - 1;
    let x = &x[..size];

    let x = std::str::from_utf8(x)?;

    let x = x.to_string();

    Ok(x)
}

pub fn get_system_code_page() -> CP {
    // Lookup code page that R is using
    let code_page = unsafe { localeCP } as u16;
    let code_page = unsafe { CP::from_raw(code_page) };
    code_page
}

#[link(name = "R", kind = "dylib")]
extern "C" {
    /// The codepage that R thinks it should be using for Windows.
    /// Should map to `winsafe::kernel::co::CP`.
    static mut localeCP: ::std::os::raw::c_uint;
}
