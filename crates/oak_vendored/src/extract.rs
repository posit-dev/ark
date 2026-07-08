use std::collections::HashSet;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use crate::ARCHIVE;

/// This MUST match `ZSTD_WINDOW_LOG` that we compress with in `vendor.rs`
const ZSTD_WINDOW_LOG: u32 = 23;

/// Extract the `{version}/` subtree of the embedded archive into `dir`
///
/// Stream-decompresses the embedded solid `tar.zst`, keeping only entries under
/// `{version}/` and stripping that prefix, so `dir` ends up holding
/// `{package}/R/...`. We stream entries straight to disk rather than holding the
/// decompressed tree in memory, so peak heap is the ~8 MB zstd window plus small
/// buffers. Files are marked read only to discourage accidental edits.
///
/// Always returns `Ok(true)` since the caller only reaches here for a resolved known
/// version.
pub(crate) fn populate(version: &str, dir: &Path) -> anyhow::Result<bool> {
    let mut archive = zstd::stream::read::Decoder::new(ARCHIVE)?;
    archive.window_log_max(ZSTD_WINDOW_LOG)?;
    let mut archive = tar::Archive::new(archive);

    let prefix = Path::new(version);

    // Parent directories we've already created
    let mut created: HashSet<PathBuf> = HashSet::new();

    for entry in archive.entries()? {
        let mut entry = entry?;
        let is_file = entry.header().entry_type().is_file();

        let path = entry.path()?;
        let Some(path) = detect_version_prefix(&path, prefix) else {
            // A different version's entry, nothing to unpack
            continue;
        };

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

    Ok(true)
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
