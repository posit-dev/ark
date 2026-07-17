use std::collections::HashSet;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use oak_cache::Cache;

use crate::compressed;

/// This MUST match `ZSTD_WINDOW_LOG` that the archive is compressed with in
/// `posit-dev/oak-r-sources`
const ZSTD_WINDOW_LOG: u32 = 23;

/// Extract the `{version}/` subtree of the downloaded archive into `dir`
///
/// Files are marked read only to discourage accidental edits.
///
/// Returns `Ok(false)` if the archive is unavailable, or if it holds no entries for
/// `version` (an R version below the archive's floor, or a gap), which we treat as
/// "source unavailable" rather than an error.
pub(crate) fn populate(version: &str, dir: &Path, compressed: &Cache) -> anyhow::Result<bool> {
    let Some(archive) = compressed::get_or_insert(compressed)? else {
        return Ok(false);
    };

    let archive = std::fs::File::open(&archive)?;
    let mut archive = zstd::stream::read::Decoder::new(archive)?;
    archive.window_log_max(ZSTD_WINDOW_LOG)?;
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
