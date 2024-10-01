//
// build.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use std::path::Path;
extern crate embed_resource;

/// [main()]
fn main() {
    // Embed an Application Manifest file on Windows.
    // Turns on UTF-8 support and declares our Windows version compatibility.
    // Documented to do nothing on non-Windows.
    // See <crates/harp/resources/manifest/harp.exe.manifest>.
    //
    // We also do this for ark.
    //
    // We don't generate a main `harp.exe` binary, but `cargo test` does generate a `harp-*.exe`
    // binary for unit testing, and those unit tests also start R and test UTF-8 related capabilities!
    // So we need that test executable to include a manifest file too.
    let resource = Path::new("resources")
        .join("manifest")
        .join("harp-manifest.rc");
    embed_resource::compile_for_everything(resource, embed_resource::NONE);
}
