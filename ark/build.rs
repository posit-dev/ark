/*
 * build.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use std::process::exit;

fn main() {
    // TODO: needs to look for R instead of guessing
    // TODO: only for #[cfg(target_os = "macos")]
    println!("cargo:rustc-link-lib=dylib=R");
    println!(
        "cargo:rustc-link-search=native=/Library/Frameworks/R.framework/Versions/Current/Resources/lib/"
    );
}
