//
// url.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use url::Url;

/// Extended URL utilities for ark.
///
/// On Windows, file URIs can have different representations of the same file.
/// Positron sends `file:///c%3A/...` (URL-encoded colon, lowercase drive) in
/// execute requests and LSP notifications. These variants can be problematic
/// when URI paths are used as HashMap keys.
///
/// This module provides normalized URI construction and parsing to ensure
/// consistent identity across subsystems (DAP breakpoints, LSP documents,
/// R code locations).
///
/// Use `ExtUrl` methods instead of `Url` methods when working with file URIs
/// that will be used as keys or need to match across different sources.
pub struct ExtUrl;

impl ExtUrl {
    /// Parse a URL string and normalize file URIs for consistent comparison.
    pub fn parse(s: &str) -> Result<Url, url::ParseError> {
        let url = Url::parse(s)?;
        Ok(Self::normalize(url))
    }

    /// Convert a file path to a normalized file URI.
    pub fn from_file_path(path: impl AsRef<std::path::Path>) -> Result<Url, ()> {
        let url = Url::from_file_path(path)?;
        Ok(Self::normalize(url))
    }

    /// Normalize a file URI for consistent comparison.
    ///
    /// On Windows, Positron sends URIs like `file:///c%3A/...` (URL-encoded
    /// colon, lowercase drive letter). By round-tripping through the filesystem
    /// path representation, we normalize encoding variants. We then uppercase
    /// the drive letter.
    #[cfg(windows)]
    pub fn normalize(uri: Url) -> Url {
        if uri.scheme() != "file" {
            return uri;
        }

        // Round-trip through filesystem path to get canonical form.
        // This decodes URL-encoded characters like %3A -> :
        let Ok(path) = uri.to_file_path() else {
            return uri;
        };

        let uri = Url::from_file_path(&path).unwrap_or(uri);
        uppercase_windows_drive_in_uri(uri)
    }

    /// No-op on non-Windows platforms.
    #[cfg(not(windows))]
    pub fn normalize(uri: Url) -> Url {
        uri
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
        let new_path = format!("/{upper}{}", &path[2..]);
        uri.set_path(&new_path);
    }

    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_non_file_unchanged() {
        let uri = Url::parse("ark://namespace/test.R").unwrap();
        let normalized = ExtUrl::normalize(uri.clone());
        assert_eq!(normalized, uri);
    }

    #[test]
    fn test_ext_url_parse_non_file() {
        let uri = ExtUrl::parse("ark://namespace/test.R").unwrap();
        assert_eq!(uri.as_str(), "ark://namespace/test.R");
    }

    #[test]
    #[cfg(not(windows))]
    fn test_normalize_is_noop_on_non_windows() {
        // On non-Windows, normalize just returns the input unchanged
        let uri = Url::parse("file:///home/user/test.R").unwrap();
        let normalized = ExtUrl::normalize(uri.clone());
        assert_eq!(normalized, uri);
    }

    #[test]
    #[cfg(not(windows))]
    fn test_ext_url_from_file_path_unix() {
        let uri = ExtUrl::from_file_path("/home/user/test.R").unwrap();
        assert_eq!(uri.as_str(), "file:///home/user/test.R");
    }

    // Windows-specific tests

    #[test]
    #[cfg(windows)]
    fn test_normalize_decodes_percent_encoded_colon() {
        // Positron sends URIs with encoded colon
        let uri = Url::parse("file:///c%3A/Users/test/file.R").unwrap();
        let normalized = ExtUrl::normalize(uri);
        assert_eq!(normalized.as_str(), "file:///C:/Users/test/file.R");
    }

    #[test]
    #[cfg(windows)]
    fn test_normalize_decodes_percent_encoded_colon_lowercase_hex() {
        // %3a (lowercase hex) variant
        let uri = Url::parse("file:///c%3a/Users/test/file.R").unwrap();
        let normalized = ExtUrl::normalize(uri);
        assert_eq!(normalized.as_str(), "file:///C:/Users/test/file.R");
    }

    #[test]
    #[cfg(windows)]
    fn test_normalize_uppercases_drive_letter() {
        let uri = Url::parse("file:///c:/Users/test/file.R").unwrap();
        let normalized = ExtUrl::normalize(uri);
        assert_eq!(normalized.as_str(), "file:///C:/Users/test/file.R");
    }

    #[test]
    #[cfg(windows)]
    fn test_normalize_preserves_uppercase_drive() {
        let uri = Url::parse("file:///C:/Users/test/file.R").unwrap();
        let normalized = ExtUrl::normalize(uri.clone());
        assert_eq!(normalized, uri);
    }

    #[test]
    #[cfg(windows)]
    fn test_normalize_preserves_spaces_encoding() {
        // Spaces should remain percent-encoded after round-trip
        let uri = Url::parse("file:///C:/Users/test%20user/my%20file.R").unwrap();
        let normalized = ExtUrl::normalize(uri);
        assert_eq!(
            normalized.as_str(),
            "file:///C:/Users/test%20user/my%20file.R"
        );
    }

    #[test]
    #[cfg(windows)]
    fn test_normalize_decodes_colon_preserves_spaces() {
        // Both encoded colon and spaces
        let uri = Url::parse("file:///c%3A/Users/test%20user/file.R").unwrap();
        let normalized = ExtUrl::normalize(uri);
        assert_eq!(normalized.as_str(), "file:///C:/Users/test%20user/file.R");
    }

    #[test]
    #[cfg(windows)]
    fn test_ext_url_parse() {
        let uri = ExtUrl::parse("file:///c%3A/Users/test/file.R").unwrap();
        assert_eq!(uri.as_str(), "file:///C:/Users/test/file.R");
    }

    #[test]
    #[cfg(windows)]
    fn test_ext_url_from_file_path() {
        let uri = ExtUrl::from_file_path("C:\\Users\\test\\file.R").unwrap();
        assert_eq!(uri.as_str(), "file:///C:/Users/test/file.R");
    }
}
