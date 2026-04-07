use std::fs::Permissions;
use std::io::Cursor;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;

use flate2::read::GzDecoder;
use tar::Archive;

use crate::directories;
use crate::directories::aether_cache_dir;
use crate::Status;

/// Returns the cached CRAN source directory for a package if it exists.
pub fn get(package: &str, version: &str) -> anyhow::Result<Option<PathBuf>> {
    let path = directories::sources_cache_dir("cran", package, version)?;

    if path.exists() {
        return Ok(Some(path));
    }

    Ok(None)
}

/// Downloads an R package's source files from CRAN, adds them to the cache, and returns
/// the cache path.
pub fn add(package: &str, version: &str) -> anyhow::Result<Status> {
    let destination = directories::sources_cache_dir("cran", package, version)?;

    // Already cached, we assume the cache is correct and just return immediately
    if destination.exists() {
        return Ok(Status::Success(destination));
    }

    let url = format!("https://cran.r-project.org/src/contrib/{package}_{version}.tar.gz");
    let response = reqwest::blocking::get(&url)?;

    // "Not on CRAN" isn't an error
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(Status::NotFound);
    }

    // But anything else is
    if !response.status().is_success() {
        return Err(anyhow::anyhow!(
            "Failed to download {url}: HTTP {status}",
            status = response.status()
        ));
    }

    extract(package, response, &destination)?;

    Ok(Status::Success(destination))
}

fn extract(
    package: &str,
    response: reqwest::blocking::Response,
    destination: &Path,
) -> anyhow::Result<()> {
    let bytes = response.bytes()?;
    let cursor = Cursor::new(bytes);
    let gz = GzDecoder::new(cursor);
    let mut archive = Archive::new(gz);

    // Looking for files under `R/`
    let prefix = format!("{package}/R/");

    // Extract into a temporary directory, then atomically rename into the cache. This
    // avoids races when multiple callers try to populate the same cache entry
    // concurrently.
    let cache = aether_cache_dir()?;
    std::fs::create_dir_all(&cache)?;
    let temporary_destination = tempfile::tempdir_in(cache)?;

    for entry in archive.entries()? {
        let mut entry = entry?;

        let path = entry.path()?;
        let path = path.to_string_lossy();

        if !path.starts_with(&prefix) {
            continue;
        }

        let Some(relative) = path.strip_prefix(&prefix) else {
            continue;
        };

        if !relative.ends_with(".R") && !relative.ends_with(".r") {
            continue;
        }

        let absolute = temporary_destination.path().join(relative);

        // Write to disk
        entry.unpack(&absolute)?;

        // Mark as read only
        std::fs::set_permissions(&absolute, Permissions::from_mode(0o444))?;
    }

    // Now rename `temporary_destination` to `destination` to atomically move it into place
    std::fs::create_dir_all(destination)?;

    match std::fs::rename(temporary_destination.path(), destination) {
        Ok(()) => {
            // Consume `TempDir` without deleting the underlying directory, since
            // we promoted it to `destination`
            let _ = temporary_destination.into_path();
            Ok(())
        },
        Err(err) => {
            // Something failed in the rename, allow `TempDir` to get cleaned up during
            // `Drop` and attempt to delete `destination` since it will be empty and we'd
            // want a future call to `add()` to try again
            if let Err(err) = std::fs::remove_dir(destination) {
                log::error!(
                    "Failed to remove {destination} after failed rename: {err:?}",
                    destination = destination.display()
                );
            }
            Err(err.into())
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_vctrs() {
        let path = match add("vctrs", "0.7.2") {
            Ok(Status::Success(path)) => path,
            Ok(Status::NotFound) => panic!("Expected Status::Success, got Status::NotFound"),
            Err(err) => panic!("Expected Status::Success, got {err:?}"),
        };

        // Check that R source files were extracted
        let entries: Vec<_> = std::fs::read_dir(&path)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(!entries.is_empty());
    }

    #[test]
    fn test_get_after_cache() {
        match add("vctrs", "0.7.2") {
            Ok(Status::Success(_)) => (),
            Ok(Status::NotFound) => panic!("Expected Status::Success, got Status::NotFound"),
            Err(err) => panic!("Expected Status::Success, got {err:?}"),
        };

        let path = get("vctrs", "0.7.2").unwrap().unwrap();
        assert!(path.exists());

        // Assert cached files are read only
        for entry in std::fs::read_dir(&path).unwrap() {
            let entry = entry.unwrap();
            let permissions = entry.metadata().unwrap().permissions();
            assert_eq!(permissions.mode() & 0o777, 0o444);
        }
    }

    #[test]
    fn test_get_not_cached() {
        let result = get("definitely_not_a_package", "0.0.0").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_not_found() {
        let result = add("definitely_not_a_package", "0.0.0");
        assert!(matches!(result, Ok(Status::NotFound)));
    }
}
