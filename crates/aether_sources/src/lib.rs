mod cran;
mod deparse;
mod directories;
mod srcref;
mod write;

use std::path::Path;
use std::path::PathBuf;

pub(crate) enum Status {
    Success(PathBuf),
    NotFound,
}

/// Returns the cached source directory for a package if it exists.
///
/// Checks known source locations and returns the first one found.
pub fn get(package: &str, version: &str) -> anyhow::Result<Option<PathBuf>> {
    if let Some(path) = srcref::get(package, version)? {
        log::trace!(
            "Found {package} {version} via srcref cache at {path}",
            path = path.display()
        );
        return Ok(Some(path));
    }

    if let Some(path) = cran::get(package, version)? {
        log::trace!(
            "Found {package} {version} via CRAN cache at {path}",
            path = path.display()
        );
        return Ok(Some(path));
    }

    if let Some(path) = deparse::get(package, version)? {
        log::trace!(
            "Found {package} {version} via deparse cache at {path}",
            path = path.display()
        );
        return Ok(Some(path));
    }

    Ok(None)
}

/// Adds a package's R source files to the cache and returns the path to them.
///
/// - Attempts to extract sources from srcref metadata via a sidecar R session.
/// - Attempts to download from CRAN.
/// - Attempts to deparse the namespace via a sidecar R session.
pub fn add(
    package: &str,
    version: &str,
    rscript: &Path,
    libpaths: Vec<&str>,
) -> anyhow::Result<Option<PathBuf>> {
    match srcref::add(package, version, rscript, &libpaths) {
        Ok(Status::Success(path)) => return Ok(Some(path)),
        Ok(Status::NotFound) => log::trace!("{package} {version} srcrefs not found"),
        Err(err) => {
            log::error!("Failed to extract {package} {version} srcrefs: {err:?}")
        },
    };

    match cran::add(package, version) {
        Ok(Status::Success(path)) => return Ok(Some(path)),
        Ok(Status::NotFound) => log::trace!("{package} {version} not found on CRAN"),
        Err(err) => {
            log::error!("Failed to download {package} {version} from CRAN: {err:?}")
        },
    };

    match deparse::add(package, version, rscript, &libpaths) {
        Ok(Status::Success(path)) => return Ok(Some(path)),
        Ok(Status::NotFound) => log::trace!("{package} {version} not found for deparsing"),
        Err(err) => {
            log::error!("Failed to deparse {package} {version}: {err:?}")
        },
    };

    Ok(None)
}
