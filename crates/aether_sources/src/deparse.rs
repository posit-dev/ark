use std::path::Path;
use std::path::PathBuf;

use crate::directories;
use crate::write;
use crate::Status;

const SCRIPT: &str = include_str!("../scripts/deparse.R");

/// Returns the cached deparse source directory for a package if it exists.
pub fn get(package: &str, version: &str) -> anyhow::Result<Option<PathBuf>> {
    let path = directories::sources_cache_dir("deparse", package, version)?;

    if path.exists() {
        return Ok(Some(path));
    }

    Ok(None)
}

/// Generates a collated R package source file from deparsing its namespace, adds it to
/// the cache, and returns the cache path.
///
/// Launches a sidecar R session to do the deparsing.
pub fn add(
    package: &str,
    version: &str,
    rscript: &Path,
    libpaths: &[&str],
) -> anyhow::Result<Status> {
    let destination = directories::sources_cache_dir("deparse", package, version)?;

    // Already cached, we assume the cache is correct and just return immediately
    if destination.exists() {
        return Ok(Status::Success(destination));
    }

    let args = &[package, version];

    // To ensure the correct R package libraries are checked
    let libpaths = libpaths.join(":");
    let env = &[("R_LIBS", libpaths.as_str())];

    let output = aether_r_process::run_script(rscript, SCRIPT, args, env)?;

    let code = output.status.code().unwrap_or(1);

    // Exit code 2 means `Status::NotFound` (not installed, version mismatch)
    if code == 2 {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::trace!("R script returned with exit code {code} for {package} {version}: {stderr}");
        return Ok(Status::NotFound);
    }

    // Any other unexpected failure
    if code != 0 {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!(
            "R script failed with exit code {code} for {package} {version}: {stderr}"
        ));
    }

    let contents = String::from_utf8(output.stdout).map_err(|err| {
        anyhow::anyhow!("R script output was not valid UTF-8 for {package} {version}: {err}")
    })?;

    let files = vec![(String::from("namespace.R"), contents)];

    write::write_to_cache(&files, &destination)?;

    Ok(Status::Success(destination))
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn test_not_found() {
//         let path = PathBuf::from("/usr/local/bin/Rscript");
//         let libpaths = &[
//             "/Users/davis/Library/R/arm64/4.5/library",
//             "/Library/Frameworks/R.framework/Versions/4.5-arm64/Resources/library",
//         ];
//         let result = add("ellmer", "0.4.0", path.as_path(), libpaths);
//         assert!(matches!(result, Ok(Status::NotFound)));
//     }
// }
