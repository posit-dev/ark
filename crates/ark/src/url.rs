//
// url.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use std::fmt;

use amalthea::wire::execute_request::CodeLocation;
use stdext::result::ResultExt;
use url::Url;

/// Extended URL utilities for ark.
///
/// # The multi-source URI reconciliation problem
///
/// File URIs for the same file arrive from four independent sources, each
/// with its own representation:
///
/// - **DAP (SetBreakpoints)**: Receives raw file paths from the frontend,
///   converted to URIs via `UrlId::from_file_path`. These are stored as
///   HashMap keys for breakpoint lookup.
///
/// - **LSP (didChange, etc.)**: Receives URIs directly from the editor
///   client, which may use non-canonical forms (e.g. percent-encoded
///   colons on Windows, or symlinked paths on macOS).
///
/// - **Execute requests**: Positron attaches a `code_location` URI that
///   comes straight from the editor's document model, again potentially
///   non-canonical.
///
/// - **R runtime**: When R evaluates `source()` or annotates code, it
///   passes URIs that went through R's `normalizePath()`, which resolves
///   symlinks to their canonical target (e.g. `/tmp` resolves to `/private/tmp`
///   on macOS), producing a path the editor never sent. More generally,
///   arbitrary R code can create source references that we may end up
///   consuming for breakpoint or debug purposes, and the paths in those
///   references may or may not be canonical.
///
/// All four sources must agree on file identity. For instance breakpoints set
/// via DAP are looked up in a HashMap keyed by URI when code is executed or
/// sourced, and invalidated when documents change via LSP.
///
/// # Design decision
///
/// We solve this by canonicalizing URIs into [`UrlId`] at every entry
/// point, rather than interning paths into opaque IDs (as rust-analyzer
/// does with its VFS `FileId` approach). Interning would be a larger
/// architectural change and is not warranted here since we only need
/// canonical keys at a handful of call sites.
///
/// Canonicalization uses `std::fs::canonicalize()` to resolve symlinks
/// (e.g. `/tmp` to `/private/tmp` on macOS), round-trips through the
/// filesystem path to normalize encoding variants (e.g. `%3A` to `:` on
/// Windows), and uppercases drive letters on Windows. When the file does
/// not exist on disk, we fall back to the original URI.
///
/// # Important: canonical URIs must not leak
///
/// [`UrlId`] is strictly for internal identity. When a URI flows back
/// to R (e.g. in `#line` directives or injected breakpoint calls) or to
/// the frontend (e.g. in DAP stack frames), always use the original raw
/// URI. The frontend (and possibly R code) expects their own URI
/// representation, and a canonical URI (e.g. `/private/tmp/...` instead of
/// `/tmp/...`) could be treated as a different file (e.g. open a new editor in
/// the frontend instead of an existing one).
///
/// A canonicalized file URI for use as a stable identity key.
///
/// Wraps a [`Url`] that has been canonicalized to resolve symlinks,
/// normalize encoding variants, and uppercase drive letters on Windows.
/// Use this type in HashMaps and anywhere file identity matters.
///
/// Construct via [`UrlId::from_url`], [`UrlId::from_file_path`], or
/// [`UrlId::parse`].
///
/// On Windows, `std::fs::canonicalize()` returns extended-length paths
/// prefixed with `\\?\` (e.g. `\\?\C:\Users\...`). Projects like Ruff
/// use the `dunce` crate to strip this prefix, but we don't need it
/// because `Url::from_file_path` already handles
/// `Prefix::VerbatimDisk` and produces a clean `file:///C:/...` URI.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UrlId(Url);

impl UrlId {
    /// Canonicalize a [`Url`] into a [`UrlId`].
    ///
    /// Resolves symlinks via `std::fs::canonicalize()` and normalizes
    /// encoding variants (e.g. `%3A` to `:` on Windows). On Windows, also
    /// uppercases the drive letter. Falls back to the original URI for
    /// non-file schemes or when the path can't be resolved.
    pub fn from_url(uri: Url) -> Self {
        if uri.scheme() != "file" {
            return Self(uri);
        }

        let Some(path) = uri.to_file_path().warn_on_err() else {
            return Self(uri);
        };

        let path = std::fs::canonicalize(&path).trace_on_err().unwrap_or(path);
        let uri = Url::from_file_path(&path)
            .map_err(|()| anyhow::anyhow!("Failed to convert path to URI: {path:?}"))
            .warn_on_err()
            .unwrap_or(uri);

        #[cfg(windows)]
        let uri = uppercase_windows_drive_in_uri(uri);

        Self(uri)
    }

    /// Convert a file path to a canonical [`UrlId`].
    ///
    /// Canonicalizes the path to resolve symlinks (e.g. `/var/folders` to
    /// `/private/var/folders` on macOS) so the URI matches what R's
    /// `normalizePath()` produces. Falls back to the original path if
    /// canonicalization fails.
    pub fn from_file_path(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let url = Url::from_file_path(path)
            .map_err(|()| anyhow::anyhow!("Failed to convert path to URL: {}", path.display()))?;
        Ok(Self::from_url(url))
    }

    /// Parse a URI string into a canonical [`UrlId`].
    pub fn parse(s: &str) -> Result<Self, url::ParseError> {
        let url = Url::parse(s)?;
        Ok(Self::from_url(url))
    }

    /// Extract a canonical [`UrlId`] from a [`CodeLocation`].
    pub fn from_code_location(loc: &CodeLocation) -> Self {
        Self::from_url(loc.uri.clone())
    }

    /// Access the inner [`Url`].
    pub fn as_url(&self) -> &Url {
        &self.0
    }
}

impl fmt::Display for UrlId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Extended URL utilities.
///
/// These operate on raw `Url` values and don't require canonicalization.
/// For identity-sensitive operations (HashMap keys, breakpoint matching),
/// use [`UrlId`] instead.
pub struct ExtUrl;

impl ExtUrl {
    /// Whether this URI should be indexed. Currently uses an exclude list:
    /// only `ark://` virtual documents are excluded since they show foreign
    /// code the user can't edit.
    pub fn is_indexable(uri: &Url) -> bool {
        !Self::is_ark_virtual_doc(uri)
    }

    /// Whether this URI should get diagnostics. Currently uses the same
    /// exclude list as [`Self::is_indexable`] but kept separate so the
    /// criteria can diverge independently.
    pub fn should_diagnose(uri: &Url) -> bool {
        !Self::is_ark_virtual_doc(uri)
    }

    /// Whether this URI points to an `ark://` virtual document (e.g. debugger
    /// vdocs showing foreign code).
    pub fn is_ark_virtual_doc(uri: &Url) -> bool {
        uri.scheme() == "ark"
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
    fn test_is_ark_virtual_doc() {
        let ark_uri = Url::parse("ark://namespace/test.R").unwrap();
        assert!(ExtUrl::is_ark_virtual_doc(&ark_uri));

        let file_uri = Url::parse("file:///home/user/test.R").unwrap();
        assert!(!ExtUrl::is_ark_virtual_doc(&file_uri));
    }

    #[test]
    fn test_is_indexable() {
        let file_uri = Url::parse("file:///home/user/test.R").unwrap();
        assert!(ExtUrl::is_indexable(&file_uri));

        let git_uri = Url::parse("git:///home/user/test.R?ref=HEAD").unwrap();
        assert!(ExtUrl::is_indexable(&git_uri));

        let ark_uri = Url::parse("ark://namespace/test.R").unwrap();
        assert!(!ExtUrl::is_indexable(&ark_uri));
    }

    #[test]
    fn test_should_diagnose() {
        let file_uri = Url::parse("file:///home/user/test.R").unwrap();
        assert!(ExtUrl::should_diagnose(&file_uri));

        let git_uri = Url::parse("git:///home/user/test.R?ref=HEAD").unwrap();
        assert!(ExtUrl::should_diagnose(&git_uri));

        let ark_uri = Url::parse("ark://namespace/test.R").unwrap();
        assert!(!ExtUrl::should_diagnose(&ark_uri));
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
    fn test_fallback_for_nonexistent_path() {
        // For paths that don't exist, canonicalization falls back to the
        // original path so the URI is unchanged.
        let uri = Url::parse("file:///nonexistent/path/test.R").unwrap();
        let id = UrlId::from_url(uri.clone());
        assert_eq!(*id.as_url(), uri);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn test_resolves_tmp_symlink() {
        // On macOS, `/tmp` is a symlink to `/private/tmp`. `UrlId` should
        // resolve it so that URIs from different sources match.
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let file = dir.path().join("test.R");
        std::fs::write(&file, "").unwrap();

        let non_canonical = Url::from_file_path(&file).unwrap();
        assert!(non_canonical.path().starts_with("/tmp/"));

        let id = UrlId::from_url(non_canonical);
        assert!(id.as_url().path().starts_with("/private/tmp/"));
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
