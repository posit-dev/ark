use std::io::Cursor;
use std::path::Path;

use flate2::read::GzDecoder;

/// Assumes `destination` exists and is file locked by the caller
pub(crate) fn cache_cran(package: &str, version: &str, destination: &Path) -> anyhow::Result<bool> {
    let response = download(package, version)?;

    // "Not on CRAN" isn't an error
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    }

    // But anything else is
    if !response.status().is_success() {
        return Err(anyhow::anyhow!(
            "Failed to download {package} {version}: HTTP {status}",
            status = response.status()
        ));
    }

    let destination = destination.join("R");
    std::fs::create_dir(&destination)?;

    extract(package, response, destination.as_path())?;

    Ok(true)
}

fn download(package: &str, version: &str) -> anyhow::Result<reqwest::blocking::Response> {
    let mirrors = ["https://cran.r-project.org", "https://cran.rstudio.com"];

    // Try released version
    let response =
        download_with_mirrors(&format!("src/contrib/{package}_{version}.tar.gz"), &mirrors)?;

    if response.status() != reqwest::StatusCode::NOT_FOUND {
        // Found it
        return Ok(response);
    }

    // Try archive
    let response = download_with_mirrors(
        &format!("src/contrib/Archive/{package}/{package}_{version}.tar.gz"),
        &mirrors,
    )?;

    // Return `response` whether or not we found something
    Ok(response)
}

fn download_with_mirrors(
    suffix: &str,
    mirrors: &[&str],
) -> anyhow::Result<reqwest::blocking::Response> {
    if mirrors.is_empty() {
        panic!("`mirrors` can't be empty.");
    }

    let mut out = None;

    for mirror in mirrors {
        let url = format!("{mirror}/{suffix}");
        let response = reqwest::blocking::get(&url)?;
        let status = response.status();

        out = Some(response);

        if status == reqwest::StatusCode::SERVICE_UNAVAILABLE {
            // Try next mirror, this one is temporarily unavailable
            continue;
        } else {
            // We got an actual response of some kind from this mirror, return it
            break;
        }
    }

    // Safety: We guarantee that there is at least 1 mirror
    Ok(out.unwrap())
}

fn extract(
    package: &str,
    response: reqwest::blocking::Response,
    destination: &Path,
) -> anyhow::Result<()> {
    // Pass response bytes of the `.tar.gz` into a gzip decoder, wrapped in a tar archive
    // reader, this abstracts away all the details, so we can just iterate over the
    // entries
    let bytes = response.bytes()?;
    let cursor = Cursor::new(bytes);
    let gz = GzDecoder::new(cursor);
    let mut archive = tar::Archive::new(gz);

    // Looking for files under `R/`
    let prefix = format!("{package}/R/");

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

        let absolute = destination.join(relative);

        // Write to disk
        entry.unpack(&absolute)?;
        crate::fs::set_readonly(&absolute)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use crate::cran::cache_cran;

    /// Requires internet access
    #[test]
    fn test_cran_r_files_exist_and_are_readonly() {
        let destination = TempDir::new().unwrap();

        let ok = cache_cran("vctrs", "0.7.2", destination.path()).unwrap();
        assert!(ok);

        let r_dir = destination.path().join("R");
        assert!(r_dir.exists());

        for entry in std::fs::read_dir(&r_dir).unwrap() {
            let entry = entry.unwrap();
            let metadata = entry.metadata().unwrap();
            assert!(metadata.permissions().readonly());
        }
    }

    #[test]
    fn test_cache_cran_not_found() {
        let destination = TempDir::new().unwrap();

        let ok = cache_cran("definitely_not_a_package", "0.0.0", destination.path()).unwrap();
        assert!(!ok);
    }
}
