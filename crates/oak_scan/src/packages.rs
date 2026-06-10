//! Filesystem-level R package discovery. Pure I/O, no salsa access. Reused by
//! both the library scanner (which walks the package folders of a library
//! directory) and the workspace scanner (which walks the workspace tree looking
//! for `DESCRIPTION` files at any depth).

use std::fs;
use std::path::Path;
use std::path::PathBuf;

use aether_url::UrlId;
use oak_package_metadata::description::Description;
use oak_package_metadata::namespace::Namespace;
use stdext::result::ResultExt;

use crate::inputs::FileEntry;

/// One package discovered on disk: its `DESCRIPTION`-derived metadata
/// plus the R files under `R/`.
#[derive(Debug)]
pub(crate) struct PackageDescriptor {
    /// URL of the `DESCRIPTION` file. This is the identity key for the
    /// `Package` entity: the same path produces the same entity across
    /// rescans, even when the package's version or files change. So a
    /// version bump updates the existing entity in place rather than
    /// minting a new one (see `Package::description_url`). The `name`
    /// can't serve as identity because two packages can declare the
    /// same `Package:` field, and dedup picks one of them per root.
    pub description_url: UrlId,
    pub name: String,
    pub version: Option<String>,
    pub namespace: Namespace,
    pub files: Vec<FileEntry>,
    pub collation: Option<Vec<String>>,
}

/// Read a candidate package directory. Returns `None` if `DESCRIPTION`
/// is missing or malformed.
pub(crate) fn read_package(dir: &Path) -> Option<PackageDescriptor> {
    let description_path = dir.join("DESCRIPTION");
    let description_text = fs::read_to_string(&description_path).ok()?;
    let description = Description::parse(&description_text).log_err()?;
    let description_url = UrlId::from_file_path(&description_path).ok()?;

    let namespace = fs::read_to_string(dir.join("NAMESPACE"))
        .ok()
        .and_then(|text| Namespace::parse(&text).log_err())
        .unwrap_or_default();

    let files = scan_r_files(&dir.join("R"));
    let collation = description.collate();

    Some(PackageDescriptor {
        description_url,
        name: description.name,
        version: Some(description.version),
        namespace,
        files,
        collation,
    })
}

/// Read every `*.R` / `*.r` file directly under `r_dir`, in alphabetical
/// order by basename. Subdirectories are skipped (the standard R package
/// layout is flat). R files that fail to read are logged at warn level
/// and skipped. Symlinks resolving to non-files (the `is_r_file` check)
/// are skipped quietly.
///
/// Returns empty for installed (library) packages: their `R/` holds the
/// lazy-load db (`<pkg>`, `<pkg>.rdb`, `<pkg>.rdx`), not `.R` sources. Only
/// source packages (the workspace scanner's input) have `R/*.R` to read.
fn scan_r_files(r_dir: &Path) -> Vec<FileEntry> {
    let mut entries: Vec<(PathBuf, String)> = Vec::new();
    let Ok(read_dir) = fs::read_dir(r_dir) else {
        // Normal for packages without an `R/` directory (data-only,
        // header-only). Don't log.
        return Vec::new();
    };

    for entry in read_dir.flatten() {
        let path = entry.path();
        if !is_r_file(&path) {
            continue;
        }
        let contents = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(err) => {
                log::warn!("Failed to read R file {}: {err}", path.display());
                continue;
            },
        };
        entries.push((path, contents));
    }

    entries.sort_by(|a, b| {
        let a_name = a.0.file_name().map(|n| n.to_ascii_lowercase());
        let b_name = b.0.file_name().map(|n| n.to_ascii_lowercase());
        a_name.cmp(&b_name)
    });

    entries
        .into_iter()
        .filter_map(|(path, contents)| {
            let url = UrlId::from_file_path(&path).ok()?;
            Some(FileEntry { url, contents })
        })
        .collect()
}

fn is_r_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let Some(ext) = path.extension() else {
        return false;
    };
    ext.eq_ignore_ascii_case("r")
}
