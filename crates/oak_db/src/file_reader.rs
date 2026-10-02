//! File reads used by Oak queries. Path resolution remains independent of I/O.

use std::fs;
use std::io;

use camino::Utf8Path;
#[cfg(any(test, feature = "fuzz"))]
use camino::Utf8PathBuf;
#[cfg(any(test, feature = "fuzz"))]
use rustc_hash::FxHashMap;

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

/// Fixtures whose files lack explicit overrides use this reader. Materialized
/// worlds use [`MapFileReader`] to serve `DESCRIPTION` text.
#[cfg(test)]
pub(crate) struct EmptyFileReader;

#[cfg(test)]
impl FileReader for EmptyFileReader {
    fn read_to_string(&self, _path: &Utf8Path) -> io::Result<String> {
        Err(io::ErrorKind::NotFound.into())
    }
}

/// Serves mapped files and reports all others absent. This supplies
/// `DESCRIPTION` text without adding a dedicated package override.
#[cfg(any(test, feature = "fuzz"))]
pub(crate) struct MapFileReader {
    files: FxHashMap<Utf8PathBuf, String>,
}

#[cfg(any(test, feature = "fuzz"))]
impl MapFileReader {
    pub(crate) fn new(files: FxHashMap<Utf8PathBuf, String>) -> Self {
        Self { files }
    }
}

#[cfg(any(test, feature = "fuzz"))]
impl FileReader for MapFileReader {
    fn read_to_string(&self, path: &Utf8Path) -> io::Result<String> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| io::ErrorKind::NotFound.into())
    }
}
