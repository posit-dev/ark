//
// build.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::process::Command;

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

    // Get the build date as a string
    let build_date = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    println!("cargo:rustc-env=BUILD_DATE={}", build_date);
}
