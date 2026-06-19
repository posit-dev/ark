//! Tagged identity for a file. The two arms encode where the file
//! lives:
//!
//! - [`FilePath::File`] wraps an [`AbsPathBuf`] (a UTF-8 absolute path
//!   with lexical normalisation applied at construction). This is the
//!   identity HashMaps key on for anything that has a filesystem
//!   representation.
//! - [`FilePath::Virtual`] wraps a [`VirtualUri`] (a non-`file:` URI
//!   preserved byte for byte). Identity is exact string equality.
//!
//! No filesystem I/O happens in construction. Bridging across symlinks
//! is the job of secondary canonical-path indexes at the specific call
//! sites that need it, never of this type.

use std::path::PathBuf;

use camino::Utf8Component;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use stdext::result::ResultExt;
use url::Url;

/// Tagged identity for a file. See module docs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FilePath {
    /// A real filesystem file. Identity is the lexically normalised
    /// absolute path.
    File(AbsPathBuf),
    /// A URI with any scheme other than `file:`. Identity is the
    /// verbatim URI.
    Virtual(VirtualUri),
}

impl FilePath {
    /// Convert a URL into a `FilePath`.
    ///
    /// Dispatches by scheme. `file:` URLs build a [`FilePath::File`];
    /// everything else builds a [`FilePath::Virtual`] that preserves
    /// the URL verbatim.
    pub fn from_url(url: &Url) -> Self {
        if url.scheme() == "file" {
            if let Some(path) = AbsPathBuf::from_url(url) {
                return Self::File(path);
            }
            // Fall through: a `file:` URL we can't extract a path from
            // stays as Virtual so the input isn't lost. Rare in practice.
        }
        Self::Virtual(VirtualUri::new(url.clone()))
    }

    /// Build a [`FilePath::File`] from a filesystem path. Returns `None`
    /// if the path can't be expressed as a UTF-8 absolute path.
    pub fn from_path_buf(path: PathBuf) -> Option<Self> {
        AbsPathBuf::from_path_buf(path).map(Self::File)
    }

    /// Parse a URI string into a [`FilePath`]. `file:` URIs become
    /// [`FilePath::File`]; everything else becomes [`FilePath::Virtual`].
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        let url = Url::parse(s)?;
        Ok(Self::from_url(&url))
    }

    /// Reconstruct a [`Url`].
    ///
    /// `File` arms rebuild a `file:` URL from the stored path; `Virtual`
    /// arms return the stored URL verbatim. Note that `File` round-trips
    /// can produce a URL that differs in bytes from the original input
    /// (drive-letter casing, encoded `:`). When that matters, store the
    /// original URL alongside in a separate field instead of relying on
    /// this method.
    pub fn to_url(&self) -> Url {
        match self {
            Self::File(path) => path.to_url(),
            Self::Virtual(uri) => uri.as_url().clone(),
        }
    }

    /// `true` for the `File` arm.
    pub fn is_file(&self) -> bool {
        matches!(self, Self::File(_))
    }

    /// Borrow the inner [`AbsPathBuf`] for the `File` arm.
    pub fn as_file(&self) -> Option<&AbsPathBuf> {
        match self {
            Self::File(p) => Some(p),
            Self::Virtual(_) => None,
        }
    }

    /// Borrow the filesystem path for the `File` arm. `None` for `Virtual`.
    pub fn as_path(&self) -> Option<&Utf8Path> {
        self.as_file().map(AbsPathBuf::as_path)
    }

    /// Borrow the inner [`VirtualUri`] for the `Virtual` arm.
    pub fn as_virtual(&self) -> Option<&VirtualUri> {
        match self {
            Self::Virtual(u) => Some(u),
            Self::File(_) => None,
        }
    }
}

impl std::fmt::Display for FilePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `File` arms format as a `file:` URL so the output matches
        // what we'd send on the wire, not as a bare path. The path
        // form is reachable via `as_path()` for callers that want it.
        match self {
            Self::File(p) => p.to_url().fmt(f),
            Self::Virtual(u) => u.fmt(f),
        }
    }
}

/// Lexically normalised absolute UTF-8 path. Identity for filesystem
/// files inside [`FilePath::File`].
///
/// Normalisation applied at construction:
/// - `.` segments dropped, `..` resolved lexically, repeated separators
///   and trailing slashes collapsed (via `Utf8Path::components()`).
/// - Windows drive letter uppercased.
///
/// No filesystem I/O. The same input produces the same `AbsPathBuf`
/// regardless of whether the file exists on disk.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AbsPathBuf(Utf8PathBuf);

impl AbsPathBuf {
    /// Build from a `file:` URL. Returns `None` for non-`file:` URLs
    /// or for `file:` URLs whose path can't be extracted as UTF-8
    /// absolute.
    pub fn from_url(url: &Url) -> Option<Self> {
        if url.scheme() != "file" {
            return None;
        }
        let path = url
            .to_file_path()
            .map_err(|()| anyhow::anyhow!("URL has no file path: {url}"))
            .warn_on_err()?;
        Self::from_path_buf(path)
    }

    /// Build from a filesystem path. Returns `None` if the path can't
    /// be represented as UTF-8 or is not absolute.
    pub fn from_path_buf(path: PathBuf) -> Option<Self> {
        let utf8 = Utf8PathBuf::from_path_buf(path)
            .map_err(|p| anyhow::anyhow!("Path is not valid UTF-8: {}", p.display()))
            .warn_on_err()?;
        Self::from_utf8_path_buf(utf8)
    }

    /// Build from a UTF-8 path. Returns `None` if the path is not
    /// absolute.
    pub fn from_utf8_path_buf(path: Utf8PathBuf) -> Option<Self> {
        if !path.is_absolute() {
            return None;
        }
        Some(Self(normalise(path)))
    }

    /// Reconstruct a `file:` URL.
    pub fn to_url(&self) -> Url {
        Url::from_file_path(self.0.as_std_path())
            .expect("AbsPathBuf is absolute: Url::from_file_path can't fail")
    }

    /// Underlying UTF-8 path.
    pub fn as_path(&self) -> &Utf8Path {
        &self.0
    }
}

impl std::fmt::Display for AbsPathBuf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// A URI with any scheme other than `file:`, preserved verbatim.
/// Identity for [`FilePath::Virtual`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VirtualUri(Url);

impl VirtualUri {
    pub fn new(url: Url) -> Self {
        Self(url)
    }

    pub fn as_url(&self) -> &Url {
        &self.0
    }

    pub fn into_url(self) -> Url {
        self.0
    }
}

impl std::fmt::Display for VirtualUri {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Lexical normalisation: collapse `.` / `..` / repeated separators /
/// trailing slashes, uppercase the Windows drive letter. Adapted from
/// the routine cargo and rust-analyzer use.
fn normalise(path: Utf8PathBuf) -> Utf8PathBuf {
    let mut components = path.components().peekable();

    // Handle prefix first and uppercase it
    let mut out = if let Some(c @ Utf8Component::Prefix(_)) = components.peek().copied() {
        components.next();
        Utf8PathBuf::from(uppercase_disk_prefix(c.as_str()))
    } else {
        Utf8PathBuf::new()
    };

    for component in components {
        match component {
            Utf8Component::Prefix(_) => unreachable!("Prefix only appears as the first component"),
            Utf8Component::RootDir => out.push(component.as_str()),
            Utf8Component::CurDir => {},
            Utf8Component::ParentDir => {
                out.pop();
            },
            Utf8Component::Normal(c) => out.push(c),
        }
    }
    out
}

/// If `prefix` is a Windows disk prefix like `c:` or `\\?\c:`,
/// uppercase the drive letter. Other prefixes (UNC, DeviceNS) pass
/// through. Operates on the prefix's string form so we don't have to
/// reconstruct from `Utf8Prefix` variants.
fn uppercase_disk_prefix(prefix: &str) -> String {
    let bytes = prefix.as_bytes();
    // `X:` somewhere in `prefix` — uppercase the drive letter byte.
    // Handles `c:`, `\\?\c:`, leaves UNC etc. alone.
    if let Some(colon_idx) = prefix.find(':') {
        if colon_idx > 0 {
            let drive_idx = colon_idx - 1;
            if bytes[drive_idx].is_ascii_lowercase() {
                let mut out = prefix.to_string();
                // Safe: the byte at drive_idx is ASCII (alphabetic).
                unsafe {
                    out.as_bytes_mut()[drive_idx] = bytes[drive_idx].to_ascii_uppercase();
                }
                return out;
            }
        }
    }
    prefix.to_string()
}

#[cfg(test)]
mod tests;
