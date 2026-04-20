use std::io;
use std::path::Path;
use std::path::PathBuf;

use etcetera::BaseStrategy;

fn cache_dir() -> anyhow::Result<PathBuf> {
    // Can fail if the home directory can't be found
    Ok(etcetera::choose_base_strategy()?.cache_dir().join("oak"))
}

pub(crate) fn sources_dir() -> anyhow::Result<PathBuf> {
    Ok(cache_dir()?.join("sources").join("v1"))
}

/// Set a file's on disk permissions to read only
pub(crate) fn set_readonly<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let mut permissions = std::fs::metadata(&path)?.permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(path, permissions)
}

pub(crate) fn copy_as_readonly<P: AsRef<Path>, Q: AsRef<Path>>(
    from: P,
    to: Q,
) -> anyhow::Result<()> {
    std::fs::copy(from.as_ref(), to.as_ref())?;
    crate::fs::set_readonly(to.as_ref())?;
    Ok(())
}

pub(crate) fn remove_dir_all_or_warn(path: &Path) {
    if let Err(err) = std::fs::remove_dir_all(path) {
        log::warn!(
            "Failed to remove directory {path}: {err:?}",
            path = path.display()
        );
    }
}
