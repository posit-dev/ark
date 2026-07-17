use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use oak_cache::Cache;

/// Release of `posit-dev/oak-r-sources` that hosts the archive
///
/// Also used as the cache key
const OAK_R_SOURCES_ASSET_VERSION: &str = "v1";

/// Newest R version present in the archive
///
/// A request newer than this is clamped to this so that people on R-devel can still
/// get mostly reliable base package information.
const OAK_R_SOURCES_LATEST_R_VERSION: &str = "4.6.1";

const ASSET: &str = "r-source.tar.zst";

/// Clamp requested R `version` to the newest version present in the archive
///
/// If `version` parses and is newer than [`OAK_R_SOURCES_LATEST_R_VERSION`], return the
/// latest.
///
/// Note that versions below the archive's floor, or gaps, aren't handled here. They cause
/// [crate::r::populate()] to return `false` instead.
pub(crate) fn clamp(version: &str) -> &str {
    match (
        parse_version(version),
        parse_version(OAK_R_SOURCES_LATEST_R_VERSION),
    ) {
        (Some(requested), Some(latest)) if requested > latest => OAK_R_SOURCES_LATEST_R_VERSION,
        _ => version,
    }
}

/// Get-or-insert the downloaded archive, returning the path to the cached
/// `r-source.tar.zst`
pub(crate) fn get_or_insert(cache: &Cache) -> anyhow::Result<Option<PathBuf>> {
    if let Some(dir) = cache.get(OAK_R_SOURCES_ASSET_VERSION) {
        return Ok(Some(dir.join(ASSET)));
    }

    let dir = cache.insert(OAK_R_SOURCES_ASSET_VERSION, populate)?;
    Ok(dir.map(|dir| dir.join(ASSET)))
}

/// Download the archive from the `posit-dev/oak-r-sources` GitHub Release into `dir`
///
/// The release URL `302`s to a signed, short-lived asset URL that can't be precomputed,
/// so we rely on `ureq` following the redirect. Returns `Ok(false)` if the asset isn't on
/// the release, which we treat as "source unavailable" rather than an error.
fn populate(dir: &Path) -> anyhow::Result<bool> {
    const HTTP_NOT_FOUND: u16 = 404;
    const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
    const GLOBAL_TIMEOUT: Duration = Duration::from_secs(60);

    let url = format!(
        "https://github.com/posit-dev/oak-r-sources/releases/download/{OAK_R_SOURCES_ASSET_VERSION}/{ASSET}"
    );

    let request = ureq::get(&url)
        .config()
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_global(Some(GLOBAL_TIMEOUT))
        .build();

    match request.call() {
        Ok(response) => {
            let mut reader = response.into_body().into_reader();
            let mut file = std::fs::File::create(dir.join(ASSET))?;
            std::io::copy(&mut reader, &mut file)?;
            Ok(true)
        },
        Err(ureq::Error::StatusCode(HTTP_NOT_FOUND)) => Ok(false),
        Err(err) => Err(err.into()),
    }
}

/// Parse `x.y.z` into a comparable tuple
fn parse_version(version: &str) -> Option<(u32, u32, u32)> {
    let mut parts = version.split('.');

    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;

    if parts.next().is_some() {
        return None;
    }

    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use crate::compressed::clamp;
    use crate::compressed::OAK_R_SOURCES_LATEST_R_VERSION;

    #[test]
    fn test_clamp_passthrough() {
        assert_eq!(clamp("4.5.0"), "4.5.0");
    }

    #[test]
    fn test_clamp_caps_above_latest() {
        assert_eq!(clamp("6.0.0"), OAK_R_SOURCES_LATEST_R_VERSION);
    }
}
