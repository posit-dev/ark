//
// version.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::collections::HashMap;
use std::env;
use std::path::PathBuf;

use anyhow::Context;
use harp::command::r_command;
use harp::command::r_home_setup;
use harp::object::RObject;
use itertools::Itertools;
use libr::SEXP;

pub struct RVersion {
    // Major version of the R installation
    pub major: u32,

    // Minor version of the R installation
    pub minor: u32,

    // Patch version of the R installation
    pub patch: u32,

    // The full path on disk to the R installation -- that is, the value R_HOME
    // would have inside an R session: > R.home()
    pub r_home: String,
}

pub fn detect_r() -> anyhow::Result<RVersion> {
    let r_home: String = r_home_setup().to_string_lossy().to_string();

    let output = r_command(|command| {
        command
            .arg("--vanilla")
            .arg("-s")
            .arg("-e")
            .arg("cat(version$major, \".\", version$minor, sep = \"\")");
    })
    .context("Failed to execute R to determine version number")?;

    let version = String::from_utf8(output.stdout)
        .context("Failed to convert R version number to a string")?
        .trim()
        .to_string();

    let version = version.split(".").map(|x| x.parse::<u32>());

    if let Some((Ok(major), Ok(minor), Ok(patch))) = version.collect_tuple() {
        Ok(RVersion {
            major,
            minor,
            patch,
            r_home,
        })
    } else {
        anyhow::bail!("Failed to extract R version");
    }
}

#[harp::register]
pub unsafe extern "C-unwind" fn ps_ark_version() -> anyhow::Result<SEXP> {
    let mut info = HashMap::<String, String>::new();
    // Set the version info in the map
    info.insert(
        String::from("version"),
        String::from(env!("CARGO_PKG_VERSION")),
    );

    // Add the current commit hash and branch; these are set by the build script (build.rs)
    info.insert(String::from("commit"), String::from(env!("BUILD_GIT_HASH")));
    info.insert(
        String::from("branch"),
        String::from(env!("BUILD_GIT_BRANCH")),
    );

    // Add the build date; this is also set by the build script
    info.insert(String::from("date"), String::from(env!("BUILD_DATE")));

    // Add the path to the kernel
    let path = env::current_exe().unwrap_or_else(|_| PathBuf::from("<unknown>"));
    info.insert(String::from("path"), path.to_string_lossy().into_owned());

    // Insert the flavor (debug or release)
    #[cfg(debug_assertions)]
    info.insert(String::from("flavor"), String::from("debug"));
    #[cfg(not(debug_assertions))]
    info.insert(String::from("flavor"), String::from("release"));

    let result = RObject::from(info);
    Ok(result.sexp)
}
