use std::io::Cursor;
use std::io::Read;

use flate2::read::GzDecoder;
use oak_fs::file_lock::FileLock;

use crate::download::Outcome;

/// Names of the R base packages, i.e. everything that ships with R and carries
/// `Priority: base` in its DESCRIPTION.
pub(crate) const BASE_PACKAGES: &[&str] = &[
    "base",
    "compiler",
    "datasets",
    "graphics",
    "grDevices",
    "grid",
    "methods",
    "parallel",
    "splines",
    "stats",
    "stats4",
    "tcltk",
    "tools",
    "utils",
];

/// Download the R source tarball for R {version} from CRAN's archive.
///
/// Base R packages (e.g. `base`, `utils`, `stats`) are not distributed at the standard
/// `src/contrib/` location on CRAN. Instead, we must retrieve them from the base R
/// sources themselves, which lives at `src/base/R-{major}/R-{version}.tar.gz`. Each
/// package is located inside that tarball at `src/library/{package}/`.
///
/// Returns `Ok(None)` if the tarball is not on CRAN (e.g. a development R version), which
/// we treat as "source unavailable" rather than an error.
pub(crate) fn download(version: &str) -> anyhow::Result<Option<Vec<u8>>> {
    let major = version
        .split('.')
        .next()
        .ok_or_else(|| anyhow::anyhow!("Invalid R version for base source download: {version}"))?;

    let mirrors = ["https://cran.r-project.org", "https://cran.rstudio.com"];
    let suffix = format!("src/base/R-{major}/R-{version}.tar.gz");

    match crate::download::download_with_mirrors(&suffix, &mirrors)? {
        Outcome::Success(response) => {
            let mut bytes = Vec::new();
            response.into_body().into_reader().read_to_end(&mut bytes)?;
            Ok(Some(bytes))
        },
        Outcome::NotFound => Ok(None),
    }
}

/// Extract a single base package's R files from the R source tarball bytes.
///
/// Writes `R-{version}/src/library/{package}/R/*.R` entries into an `R/` folder inside
/// the directory `destination_lock` lives in. Files are marked read only to match the
/// rest of the cache.
pub(crate) fn extract(
    package: &str,
    version: &str,
    bytes: &[u8],
    destination_lock: &FileLock,
) -> anyhow::Result<()> {
    let destination = destination_lock.parent().join("R");
    std::fs::create_dir(&destination)?;

    let cursor = Cursor::new(bytes);
    let gz = GzDecoder::new(cursor);
    let mut archive = tar::Archive::new(gz);

    let prefix = format!("R-{version}/src/library/{package}/R/");

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;

        let Some(relative) = path.strip_prefix(&prefix).ok() else {
            continue;
        };

        if relative
            .extension()
            .is_none_or(|ext| ext != "R" && ext != "r")
        {
            continue;
        }

        let absolute = destination.join(relative);

        // Some base packages (e.g. `utils`) have platform-specific subdirs under `R/`
        // like `R/windows/` and `R/unix/` (their `Makefile` handles them at install
        // time). Create parents if one is required so `unpack()` can write nested files.
        if let Some(parent) = relative.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(destination.join(parent))?;
        }

        entry.unpack(&absolute)?;
        crate::fs::set_readonly(&absolute)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use oak_fs::file_lock::Filesystem;
    use tempfile::TempDir;

    use crate::base::download;
    use crate::base::extract;

    /// Requires internet access and downloads a large tarball of the R sources
    #[ignore = "Downloads a 40mb tarball"]
    #[test]
    fn test_base_download_and_extract() {
        let bytes = download("4.5.0").unwrap().expect("R 4.5.0 source to exist");

        let destination_tempdir = TempDir::new().unwrap();
        let destination = Filesystem::new(destination_tempdir.path().to_path_buf());
        let destination_lock = destination.open_rw_exclusive_create(".lock").unwrap();

        extract("utils", "4.5.0", &bytes, &destination_lock).unwrap();

        // Spot check: `utils` has a well-known `help.R` file
        let help = destination_lock.parent().join("R").join("help.R");
        assert!(help.exists());
        assert!(help.metadata().unwrap().permissions().readonly());
    }

    #[test]
    fn test_base_download_unknown_version_returns_none() {
        let bytes = download("0.0.0").unwrap();
        assert!(bytes.is_none());
    }
}
