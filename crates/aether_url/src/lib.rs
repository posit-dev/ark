//
// lib.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use std::fmt;

use stdext::result::ResultExt;
use url::Url;

/// Lexically normalised file URL identity.
///
/// Internal identity key for files received from any source (LSP, DAP,
/// scanner, R runtime). Constructed via the same lexical normalisation
/// at every entry point so that two paths the editor considers "the
/// same file" produce the same [`UrlId`].
///
/// What we normalise: drive-letter casing on Windows; percent-encoding
/// of `:` (decoded via `Url -> PathBuf -> Url` round-trip). No I/O:
/// `std::fs::canonicalize()`, no symlink resolution. The same input URI
/// produces the same [`UrlId`] whether or not the file exists on disk.
///
/// # Bridging across symlinks
///
/// R's `normalizePath()` resolves symlinks on its own. A srcref URI
/// from the R runtime may name `/private/tmp/foo.R` while the editor
/// sent us `/tmp/foo.R`. The two don't compare equal, so a `HashMap`
/// keyed on `UrlId` treats them as separate files. Code that needs to
/// match a srcref URI back to an open document or a breakpoint should
/// maintain a secondary index of `fs::canonicalize`d paths and fall
/// back to it on a primary miss.
/// [`crate::dap::dap_state::BreakpointMap`] in `ark` does this for
/// breakpoints.
///
/// # Important: don't leak normalised URIs back out
///
/// Even though [`UrlId`] no longer fs-canonicalises, it still
/// uppercases the Windows drive letter and decodes the percent-encoded
/// colon. When sending URIs back to the editor or to R, prefer the
/// original bytes the frontend sent. The frontend treats a URI as the
/// editor's identity for the file; a normalised form may look like a
/// different file to it (e.g. open a new editor pane).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UrlId(Url);

impl UrlId {
    /// Lexically normalise a [`Url`] into a [`UrlId`].
    ///
    /// Decodes encoding variants (e.g. `%3A` to `:` on Windows) and
    /// uppercases the Windows drive letter. Does no filesystem I/O.
    /// Non-`file:` URLs (`ark://`, `untitled:`, ...) pass through
    /// untouched.
    pub fn from_url(uri: Url) -> Self {
        if uri.scheme() != "file" {
            return Self(uri);
        }

        // Round-trip through `PathBuf` so the URI form matches what
        // `Url::from_file_path` produces (decoded `%3A`, etc.). Skip
        // on error, we let pathological URIs flow through unchanged.
        let uri = match uri.to_file_path().warn_on_err() {
            Some(path) => Url::from_file_path(&path)
                .map_err(|()| anyhow::anyhow!("Failed to convert path to URI: {path:?}"))
                .warn_on_err()
                .unwrap_or(uri),
            None => uri,
        };

        #[cfg(windows)]
        let uri = uppercase_windows_drive_in_uri(uri);

        Self(uri)
    }

    /// Build a [`UrlId`] from a filesystem path.
    ///
    /// Same lexical normalisation as [`Self::from_url`], no filesystem
    /// I/O. Errors only if `path` can't be expressed as a URL (e.g.
    /// not absolute on platforms that require it).
    pub fn from_file_path(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let url = Url::from_file_path(path)
            .map_err(|()| anyhow::anyhow!("Failed to convert path to URL: {}", path.display()))?;
        Ok(Self::from_url(url))
    }

    /// Parse a URI string into a [`UrlId`].
    pub fn parse(s: &str) -> Result<Self, url::ParseError> {
        let url = Url::parse(s)?;
        Ok(Self::from_url(url))
    }

    /// Access the inner [`Url`].
    pub fn as_url(&self) -> &Url {
        &self.0
    }

    /// Whether this URL points at a filesystem file (`file:` scheme).
    /// Returns `false` for virtual documents like `untitled:` (unsaved
    /// buffers) and `ark:` (synthesized sources from R). Callers that
    /// might receive virtual URLs should gate on this before reaching for
    /// [`Self::to_file_path`].
    pub fn is_file(&self) -> bool {
        self.0.scheme() == "file"
    }

    /// Filesystem path corresponding to this URL.
    ///
    /// Errors for non-`file:` URLs (untitled buffers, custom schemes) and
    /// for `file:` URLs whose path can't be reconstructed (rare). Callers
    /// that handle virtual documents should check [`Self::is_file`] first.
    pub fn to_file_path(&self) -> anyhow::Result<std::path::PathBuf> {
        self.0
            .to_file_path()
            .map_err(|()| anyhow::anyhow!("URL has no filesystem path: {}", self.0))
    }
}

impl fmt::Display for UrlId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Uppercase the drive letter in a Windows file URI for consistent hashing.
#[cfg(windows)]
fn uppercase_windows_drive_in_uri(mut uri: Url) -> Url {
    let path = uri.path();
    let mut chars = path.chars();

    // Match pattern: "/" + drive letter + ":"
    let drive = match (chars.next(), chars.next(), chars.next()) {
        (Some('/'), Some(drive), Some(':')) if drive.is_ascii_alphabetic() => drive,
        _ => return uri,
    };

    let upper = drive.to_ascii_uppercase();

    if drive != upper {
        let new_path = format!("/{upper}:{}", &path[3..]);
        uri.set_path(&new_path);
    }

    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_non_file_unchanged() {
        let uri = Url::parse("ark://namespace/test.R").unwrap();
        let id = UrlId::from_url(uri.clone());
        assert_eq!(*id.as_url(), uri);
    }

    #[test]
    fn test_parse_non_file() {
        let id = UrlId::parse("ark://namespace/test.R").unwrap();
        assert_eq!(id.as_url().as_str(), "ark://namespace/test.R");
    }

    #[test]
    fn test_equality() {
        let id1 = UrlId::parse("file:///home/user/test.R").unwrap();
        let id2 = UrlId::parse("file:///home/user/test.R").unwrap();
        assert_eq!(id1, id2);

        let id3 = UrlId::parse("file:///home/user/other.R").unwrap();
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_display() {
        let id = UrlId::parse("file:///home/user/test.R").unwrap();
        assert_eq!(format!("{id}"), "file:///home/user/test.R");
    }

    #[test]
    #[cfg(not(windows))]
    fn test_nonexistent_path_unchanged() {
        // Construction is lexical-only, so nonexistent paths flow
        // through unchanged.
        let uri = Url::parse("file:///nonexistent/path/test.R").unwrap();
        let id = UrlId::from_url(uri.clone());
        assert_eq!(*id.as_url(), uri);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn test_does_not_resolve_symlinks() {
        // On macOS, `/tmp` is a symlink to `/private/tmp`. `UrlId` does
        // *not* resolve it; same input bytes produce the same output
        // regardless of the symlink graph on disk. Bridging across
        // symlinked names is the job of secondary canonical indexes at
        // specific seams (e.g. the DAP breakpoint store), not of
        // construction.
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let file = dir.path().join("test.R");
        std::fs::write(&file, "").unwrap();

        let original = Url::from_file_path(&file).unwrap();
        assert!(original.path().starts_with("/tmp/"));

        let id = UrlId::from_url(original.clone());
        assert_eq!(*id.as_url(), original);
        assert!(id.as_url().path().starts_with("/tmp/"));
    }

    #[test]
    #[cfg(not(windows))]
    fn test_from_file_path_unix() {
        let id = UrlId::from_file_path("/home/user/test.R").unwrap();
        assert_eq!(id.as_url().as_str(), "file:///home/user/test.R");
    }

    // Windows-specific tests

    #[test]
    #[cfg(windows)]
    fn test_decodes_percent_encoded_colon() {
        // Positron sends URIs with encoded colon
        let uri = Url::parse("file:///c%3A/Users/test/file.R").unwrap();
        let id = UrlId::from_url(uri);
        assert_eq!(id.as_url().as_str(), "file:///C:/Users/test/file.R");
    }

    #[test]
    #[cfg(windows)]
    fn test_decodes_percent_encoded_colon_lowercase_hex() {
        // %3a (lowercase hex) variant
        let uri = Url::parse("file:///c%3a/Users/test/file.R").unwrap();
        let id = UrlId::from_url(uri);
        assert_eq!(id.as_url().as_str(), "file:///C:/Users/test/file.R");
    }

    #[test]
    #[cfg(windows)]
    fn test_uppercases_drive_letter() {
        let uri = Url::parse("file:///c:/Users/test/file.R").unwrap();
        let id = UrlId::from_url(uri);
        assert_eq!(id.as_url().as_str(), "file:///C:/Users/test/file.R");
    }

    #[test]
    #[cfg(windows)]
    fn test_preserves_uppercase_drive() {
        let uri = Url::parse("file:///C:/Users/test/file.R").unwrap();
        let id = UrlId::from_url(uri.clone());
        assert_eq!(*id.as_url(), uri);
    }

    #[test]
    #[cfg(windows)]
    fn test_preserves_spaces_encoding() {
        // Spaces should remain percent-encoded after round-trip
        let uri = Url::parse("file:///C:/Users/test%20user/my%20file.R").unwrap();
        let id = UrlId::from_url(uri);
        assert_eq!(
            id.as_url().as_str(),
            "file:///C:/Users/test%20user/my%20file.R"
        );
    }

    #[test]
    #[cfg(windows)]
    fn test_decodes_colon_preserves_spaces() {
        // Both encoded colon and spaces
        let uri = Url::parse("file:///c%3A/Users/test%20user/file.R").unwrap();
        let id = UrlId::from_url(uri);
        assert_eq!(id.as_url().as_str(), "file:///C:/Users/test%20user/file.R");
    }

    #[test]
    #[cfg(windows)]
    fn test_parse_windows() {
        let id = UrlId::parse("file:///c%3A/Users/test/file.R").unwrap();
        assert_eq!(id.as_url().as_str(), "file:///C:/Users/test/file.R");
    }

    #[test]
    #[cfg(windows)]
    fn test_from_file_path_windows() {
        let id = UrlId::from_file_path("C:\\Users\\test\\file.R").unwrap();
        assert_eq!(id.as_url().as_str(), "file:///C:/Users/test/file.R");
    }
}
