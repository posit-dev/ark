//! Cache for base R package sources
//!
//! Base R packages don't have srcrefs and aren't available standalone from a CRAN mirror,
//! instead they are part of the entire R tarball on CRAN, which is over 100MB per R
//! version. Instead, we create minimal and compressed `r-source.tar.zst` archives that
//! contain the R sources for every base package from R 4.2.0 up to
//! `OAK_R_SOURCES_LATEST_R_VERSION`. These are hosted as GitHub Releases at
//! `posit-dev/oak-r-sources`, with a new release every R version.
//!
//! We download the `r-source.tar.zst` once per [OAK_R_SOURCES_RELEASE_VERSION] and store
//! it in the `cache` under `{OAK_R_SOURCES_RELEASE_VERSION}_archive/`, and then we
//! extract each R version's base package sources from that, and store them in the `cache`
//! under `{OAK_R_SOURCES_RELEASE_VERSION}_{version}/`.
//!
//! Note that we can't use a folder structure of
//! `{OAK_R_SOURCES_RELEASE_VERSION}/archive/` and
//! `{OAK_R_SOURCES_RELEASE_VERSION}/{version}/` even though that would be nicer
//! cosmetically due to the way [Cache] cleanup works. It assumes the entries under
//! `source/v1/r/` are a flat set of folders.

use std::collections::HashSet;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use oak_cache::Cache;

/// Release of `posit-dev/oak-r-sources` that hosts the archive
///
/// Also used as the cache key
const OAK_R_SOURCES_RELEASE_VERSION: &str = "v1";

const OAK_R_SOURCES_ASSET_NAME: &str = "r-source.tar.zst";

/// Newest R version present in the archive
///
/// A request newer than this is clamped to this so that people on R-devel or old ark can
/// still get mostly reliable base package information.
const OAK_R_SOURCES_LATEST_R_VERSION: &str = "4.6.1";

/// This MUST match the `ZSTD_WINDOW_LOG` that the archive is compressed with in
/// `posit-dev/oak-r-sources`
const OAK_R_SOURCES_ZSTD_WINDOW_LOG: u32 = 23;

#[derive(Debug)]
pub(crate) struct RCache {
    cache: Cache,
}

impl RCache {
    pub(crate) fn open(root: &str) -> anyhow::Result<Self> {
        Ok(Self {
            cache: Cache::open(root)?,
        })
    }

    pub(crate) fn open_in(root: PathBuf) -> anyhow::Result<Self> {
        Ok(Self {
            cache: Cache::open_in(root)?,
        })
    }

    pub(crate) fn get(&self, version: &str) -> Option<PathBuf> {
        let version = clamp(version);
        let key = format!("{OAK_R_SOURCES_RELEASE_VERSION}_{version}");
        self.cache.get(&key)
    }

    pub(crate) fn insert(&self, version: &str) -> Option<PathBuf> {
        let version = clamp(version);
        let key = format!("{OAK_R_SOURCES_RELEASE_VERSION}_{version}");

        let archive = self.archive()?;

        self.cache
            .insert(&key, |dir| {
                extract(version, dir, &archive.join(OAK_R_SOURCES_ASSET_NAME))
            })
            .unwrap_or_else(|err| {
                log::error!("Failed to extract R {version} from archive: {err:?}");
                None
            })
    }

    /// Get or insert the `archive`, i.e. the `r-source.tar.zst` to extract from
    fn archive(&self) -> Option<PathBuf> {
        let key = format!("{OAK_R_SOURCES_RELEASE_VERSION}_archive");

        if let Some(archive) = self.cache.get(&key) {
            return Some(archive);
        }

        match self.cache.insert(&key, populate) {
            Ok(Some(archive)) => Some(archive),
            Ok(None) => {
                log::error!(
                    "Failed to download oak-r-sources {OAK_R_SOURCES_RELEASE_VERSION} archive"
                );
                None
            },
            Err(err) => {
                log::error!("Failed to download oak-r-sources {OAK_R_SOURCES_RELEASE_VERSION} archive: {err:?}");
                None
            },
        }
    }
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
        "https://github.com/posit-dev/oak-r-sources/releases/download/{OAK_R_SOURCES_RELEASE_VERSION}/{OAK_R_SOURCES_ASSET_NAME}"
    );

    let request = ureq::get(&url)
        .config()
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_global(Some(GLOBAL_TIMEOUT))
        .build();

    match request.call() {
        Ok(response) => {
            let mut reader = response.into_body().into_reader();
            let mut file = std::fs::File::create(dir.join(OAK_R_SOURCES_ASSET_NAME))?;
            std::io::copy(&mut reader, &mut file)?;
            Ok(true)
        },
        Err(ureq::Error::StatusCode(HTTP_NOT_FOUND)) => Ok(false),
        Err(err) => Err(err.into()),
    }
}

/// Extract the `{version}/` subtree of the downloaded archive into `dir`
///
/// Files are marked read only to discourage accidental edits.
///
/// Returns `Ok(false)` if the archive holds no entries for `version` (an R version below
/// the archive's floor, or a gap), which we treat as "source unavailable" rather than an
/// error.
fn extract(version: &str, dir: &Path, archive: &Path) -> anyhow::Result<bool> {
    let archive = std::fs::File::open(archive)?;
    let mut archive = zstd::stream::read::Decoder::new(archive)?;
    archive.window_log_max(OAK_R_SOURCES_ZSTD_WINDOW_LOG)?;
    let mut archive = tar::Archive::new(archive);

    let prefix = Path::new(version);

    // Parent directories we've already created
    let mut created: HashSet<PathBuf> = HashSet::new();

    // Have we ever seen an entry for the requested `version`?
    let mut seen_version = false;

    for entry in archive.entries()? {
        let mut entry = entry?;
        let is_file = entry.header().entry_type().is_file();

        let path = entry.path()?;
        let Some(path) = detect_version_prefix(&path, prefix) else {
            // A different version's entry, nothing to unpack
            continue;
        };

        seen_version = true;
        let destination = dir.join(path);

        // We must create parent directories before unpacking into them. We remember ones
        // we've already created to avoid thousands of redundant `create_dir_all()` calls.
        if let Some(parent) = destination.parent() {
            if !created.contains(parent) {
                std::fs::create_dir_all(parent)?;
                created.insert(parent.to_path_buf());
            }
        }

        entry.unpack(&destination)?;

        if is_file {
            set_readonly(&destination)?;
        }
    }

    Ok(seen_version)
}

/// Detect archive entries with a `{version}/` prefix
///
/// - Returns `Some(path)` stripped of the `{version}/` prefix if it existed
/// - Returns `None` for an entry belonging to a different version
fn detect_version_prefix<'path>(path: &'path Path, version: &Path) -> Option<&'path Path> {
    let path = path.strip_prefix(version).ok()?;

    // We need a file left over!
    if path.as_os_str().is_empty() {
        return None;
    }

    // No `../` or `./` shenanigans allowed
    if !path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return None;
    }

    Some(path)
}

/// Mark a file as read only
fn set_readonly(path: &Path) -> std::io::Result<()> {
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(path, permissions)
}

/// Clamp requested R `version` to the newest version present in the archive
///
/// If `version` parses and is newer than [`OAK_R_SOURCES_LATEST_R_VERSION`], return the
/// latest.
///
/// Note that versions below the archive's floor, or gaps, aren't handled here. They cause
/// [extract()] to return `false` instead.
fn clamp(version: &str) -> &str {
    match (
        parse_version(version),
        parse_version(OAK_R_SOURCES_LATEST_R_VERSION),
    ) {
        (Some(requested), Some(latest)) if requested > latest => OAK_R_SOURCES_LATEST_R_VERSION,
        _ => version,
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
    use crate::r::clamp;
    use crate::r::OAK_R_SOURCES_LATEST_R_VERSION;

    #[test]
    fn test_clamp_passthrough() {
        assert_eq!(clamp("4.5.0"), "4.5.0");
    }

    #[test]
    fn test_clamp_caps_above_latest() {
        assert_eq!(clamp("6.0.0"), OAK_R_SOURCES_LATEST_R_VERSION);
    }
}
