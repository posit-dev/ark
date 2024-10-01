//
// build.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::path::Path;
use std::process::Command;
extern crate embed_resource;

fn main() {
    // Attempt to use `git rev-parse HEAD` to get the current git hash. If this
    // fails, we'll just use the string "<unknown>" to indicate that the git hash
    // could not be determined..
    let git_hash = match Command::new("git")
        .args(&["rev-parse", "--short", "HEAD"])
        .output()
    {
        Ok(output) => String::from_utf8(output.stdout).unwrap(),
        Err(_) => String::from("<unknown>"),
    };
    println!("cargo:rustc-env=BUILD_GIT_HASH={}", git_hash);

    let git_branch = match Command::new("git")
        .args(&["branch", "--show-current"])
        .output()
    {
        Ok(output) => String::from_utf8(output.stdout).unwrap(),
        Err(_) => String::from("<unknown>"),
    };
    println!("cargo:rustc-env=BUILD_GIT_BRANCH={}", git_branch);

    // Get the build date as a string
    let build_date = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    println!("cargo:rustc-env=BUILD_DATE={}", build_date);

    // Embed an Application Manifest file on Windows.
    // Turns on UTF-8 support and declares our Windows version compatibility.
    // Documented to do nothing on non-Windows.
    // We also do this for harp to support its unit tests.
    //
    // We can't just use `compile()`, as that uses `cargo:rustc-link-arg-bins`,
    // which targets the main `ark.exe` (good) but not the test binaries (bad).
    // We need the application manifest to get embedded into the ark/harp test
    // binaries too, so that the instance of R started by our tests also has
    // UTF-8 support.
    //
    // We can't use `compile_for_tests()` because `cargo:rustc-link-arg-tests`
    // only targets integration tests right now, not unit tests.
    // https://github.com/rust-lang/cargo/issues/10937
    //
    // Instead we use `compile_for_everything()` which uses the kitchen sink
    // instruction of `cargo:rustc-link-arg`, and that seems to work.
    // https://github.com/nabijaczleweli/rust-embed-resource/issues/69
    let resource = Path::new("resources")
        .join("manifest")
        .join("ark-manifest.rc");
    embed_resource::compile_for_everything(resource, embed_resource::NONE);
}
