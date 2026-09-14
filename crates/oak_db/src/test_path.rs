//! Test and fuzz paths need no files on disk. Their rendered names omit the
//! platform-specific root so assertions work on Windows and Unix.

use aether_path::FilePath;
use url::Url;

pub(crate) fn file_path(name: &str) -> FilePath {
    // Windows file URLs need a drive letter for `Url::to_file_path()`.
    let url = if cfg!(windows) {
        Url::parse(&format!("file:///C:/{name}")).unwrap()
    } else {
        Url::parse(&format!("file:///{name}")).unwrap()
    };
    FilePath::from_url(&url)
}

/// Omit the file URL root, including the synthetic Windows drive letter, so
/// assertions use the same relative names on every platform.
pub(crate) fn path_name(path: &FilePath) -> String {
    let url = path.to_url();
    let path = url.path();
    let prefix = if cfg!(windows) { "/C:/" } else { "/" };
    path.strip_prefix(prefix).unwrap_or(path).to_string()
}
