use std::path::Path;

use crate::download::download_with_mirrors;
use crate::download::Outcome;
use crate::download::MIRRORS;
use crate::extract;

/// Download the R source tarball for R `version` from CRAN and unpack it into `dir`
///
/// The base R packages aren't distributed at the standard `src/contrib/` location. They
/// live inside the R source tarball at `src/base/R-{major}/R-{version}.tar.gz`. The
/// tarball's top-level `R-{version}/` directory is stripped, so they land under
/// `dir/src/library/{name}/`.
///
/// Returns `Ok(false)` if the tarball isn't on CRAN (e.g. a development R version), which
/// we treat as "source unavailable" rather than an error.
pub(crate) fn populate(version: &str, dir: &Path) -> anyhow::Result<bool> {
    let major = version
        .split('.')
        .next()
        .ok_or_else(|| anyhow::anyhow!("Invalid R version for source download: {version}"))?;

    let suffix = format!("src/base/R-{major}/R-{version}.tar.gz");

    match download_with_mirrors(&suffix, MIRRORS)? {
        Outcome::Success(response) => {
            extract::extract(response.into_body().into_reader(), dir)?;
            Ok(true)
        },
        Outcome::NotFound => Ok(false),
    }
}
