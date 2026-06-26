use std::path::Path;

use crate::download::download_with_mirrors;
use crate::download::Outcome;
use crate::download::MIRRORS;
use crate::extract;

/// Download an R package's source tarball from CRAN and unpack it into `dir`
///
/// The tarball's top-level `{name}/` directory is stripped, so the package's files land
/// directly under `dir` (e.g. `dir/R/`, `dir/DESCRIPTION`).
///
/// Returns `Ok(false)` if the package isn't on CRAN, which we treat as "source
/// unavailable" rather than an error.
pub(crate) fn populate(name: &str, version: &str, dir: &Path) -> anyhow::Result<bool> {
    match download(name, version)? {
        Outcome::Success(response) => {
            extract::extract(response.into_body().into_reader(), dir)?;
            Ok(true)
        },
        Outcome::NotFound => Ok(false),
    }
}

fn download(name: &str, version: &str) -> anyhow::Result<Outcome> {
    // Try released version
    let outcome = download_with_mirrors(&format!("src/contrib/{name}_{version}.tar.gz"), MIRRORS)?;

    if matches!(outcome, Outcome::Success(_)) {
        return Ok(outcome);
    }

    // Try archive
    download_with_mirrors(
        &format!("src/contrib/Archive/{name}/{name}_{version}.tar.gz"),
        MIRRORS,
    )
}
