use std::path::Path;

use flate2::read::GzDecoder;
use oak_fs::file_lock::FileLock;

use crate::download::download_with_mirrors;
use crate::download::Outcome;

/// Downloads an R package's source files from CRAN if possible and adds them to the cache
/// at the parent folder containing `destination_lock`
pub(crate) fn cache_cran(
    package: &str,
    version: &str,
    destination_lock: &FileLock,
) -> anyhow::Result<bool> {
    match download(package, version) {
        Ok(Outcome::Success(response)) => {
            let destination = destination_lock.parent().join("R");
            std::fs::create_dir(&destination)?;
            extract(package, response, destination.as_path())?;
            Ok(true)
        },

        Ok(Outcome::NotFound) => {
            // "Not on CRAN" isn't an error
            Ok(false)
        },

        Err(err) => Err(anyhow::anyhow!(
            "Failed to download {package} {version}: {err:?}"
        )),
    }
}

fn download(package: &str, version: &str) -> anyhow::Result<Outcome> {
    let mirrors = ["https://cran.r-project.org", "https://cran.rstudio.com"];

    // Try released version
    let outcome =
        download_with_mirrors(&format!("src/contrib/{package}_{version}.tar.gz"), &mirrors)?;

    if matches!(outcome, Outcome::Success(_)) {
        return Ok(outcome);
    }

    // Try archive
    download_with_mirrors(
        &format!("src/contrib/Archive/{package}/{package}_{version}.tar.gz"),
        &mirrors,
    )
}

fn extract(
    package: &str,
    response: ureq::http::Response<ureq::Body>,
    destination: &Path,
) -> anyhow::Result<()> {
    // Stream the response body through a gzip decoder wrapped in a tar archive reader
    // so we can just iterate over entries. `into_reader()` is unlimited by default.
    let reader = response.into_body().into_reader();
    let gz = GzDecoder::new(reader);
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
    use oak_fs::file_lock::Filesystem;
    use tempfile::TempDir;

    use crate::cran::cache_cran;

    /// Requires internet access
    #[test]
    fn test_cran_r_files_exist_and_are_readonly() {
        let destination_tempdir = TempDir::new().unwrap();
        let destination = Filesystem::new(destination_tempdir.path().to_path_buf());
        let destination_lock = destination.open_rw_exclusive_create(".lock").unwrap();

        let ok = cache_cran("vctrs", "0.7.2", &destination_lock).unwrap();
        assert!(ok);

        let r_dir = destination_lock.parent().join("R");
        assert!(r_dir.exists());

        for entry in std::fs::read_dir(&r_dir).unwrap() {
            let entry = entry.unwrap();
            let metadata = entry.metadata().unwrap();
            assert!(metadata.permissions().readonly());
        }
    }

    #[test]
    fn test_cache_cran_not_found() {
        let destination_tempdir = TempDir::new().unwrap();
        let destination = Filesystem::new(destination_tempdir.path().to_path_buf());
        let destination_lock = destination.open_rw_exclusive_create(".lock").unwrap();

        let ok = cache_cran("definitely_not_a_package", "0.0.0", &destination_lock).unwrap();
        assert!(!ok);
    }
}
