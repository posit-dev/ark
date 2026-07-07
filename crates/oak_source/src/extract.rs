use std::collections::HashSet;
use std::io::Read;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use flate2::read::GzDecoder;

/// Unpack a gzipped tarball into `dir`, dropping its top-level directory and marking
/// every file read only
///
/// CRAN package tarballs and R source tarballs both wrap their content in a single
/// top-level directory (`{package}/` or `R-{version}/`). We strip it so the content lands
/// directly under `dir`.
///
/// Files are marked as read only to discourage accidental edits.
pub(crate) fn extract(reader: impl Read, dir: &Path) -> anyhow::Result<()> {
    let gz = GzDecoder::new(reader);
    let mut archive = tar::Archive::new(gz);

    // Parent directories we've already created
    let mut created: HashSet<PathBuf> = HashSet::new();

    for entry in archive.entries()? {
        let mut entry = entry?;
        let is_file = entry.header().entry_type().is_file();

        let path = entry.path()?.into_owned();
        let Some(relative) = strip_top_level(&path) else {
            // The top-level directory entry itself, or an unsafe path, nothing to unpack
            continue;
        };

        let destination = dir.join(relative);

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

    Ok(())
}

/// Strip the single top-level directory from a tarball entry path
///
/// Returns `None` for the top-level directory entry itself, or for any unsafe path
/// (absolute, or containing `..`) that could escape the destination.
fn strip_top_level(path: &Path) -> Option<&Path> {
    let mut components = path.components();
    components.next()?;

    let rest = components.as_path();

    if rest.as_os_str().is_empty() {
        // The top-level directory entry itself
        return None;
    }

    if !rest.components().all(|c| matches!(c, Component::Normal(_))) {
        // Something would be strange here!
        return None;
    }

    Some(rest)
}

/// Mark a file as read only
fn set_readonly(path: &Path) -> std::io::Result<()> {
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(path, permissions)
}
