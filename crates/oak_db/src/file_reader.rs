//! File reads used by Oak queries. Path resolution remains independent of I/O.

use std::fs;
use std::io;

use camino::Utf8Path;

/// Supplies file contents to queries when no editor or namespace override is set.
///
/// Readers must be shared by database snapshots. Changes to their contents must
/// be accompanied by the corresponding file or package revision bump, just like
/// filesystem changes. The reader itself is fixed for the database's lifetime.
pub(crate) trait FileReader: Send + Sync {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String>;
}

pub(crate) struct DiskFileReader;

impl FileReader for DiskFileReader {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        fs::read_to_string(path)
    }
}

/// Fixtures supply source and namespace overrides; all other files are absent.
#[cfg(any(test, feature = "fuzz"))]
pub(crate) struct EmptyFileReader;

#[cfg(any(test, feature = "fuzz"))]
impl FileReader for EmptyFileReader {
    fn read_to_string(&self, _path: &Utf8Path) -> io::Result<String> {
        Err(io::ErrorKind::NotFound.into())
    }
}
