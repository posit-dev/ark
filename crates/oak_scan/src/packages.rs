//! Filesystem-level R package discovery. Pure I/O, no salsa access. Reused by
//! both the library scanner (which walks the package folders of a library
//! directory) and the workspace scanner (which walks the workspace tree looking
//! for `DESCRIPTION` files at any depth).

use std::fs;
use std::path::Path;
use std::path::PathBuf;

use aether_url::UrlId;
use ignore::WalkBuilder;
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
    path.is_file() && oak_core::is_r_file(path)
}

/// Walk a workspace root, returning every discovered package and every
/// top-level R script that isn't inside a package directory.
///
/// `DESCRIPTION` files are looked up at any depth, honouring `.gitignore`
/// and `.ignore` and skipping hidden directories. A package's R/ files
/// are scoped to `{pkg_dir}/R/*.R`. R files that fall inside any
/// discovered package directory but outside its `R/` (e.g. tests/,
/// vignettes/, inst/) are excluded from `scripts`. Everything else with
/// an `.R` extension becomes a top-level script on the workspace root.
pub(crate) fn scan_workspace(root: &Path) -> (Vec<PackageDescriptor>, Vec<FileEntry>) {
    let description_dirs = collect_description_dirs(root);

    let packages: Vec<PackageDescriptor> = description_dirs
        .iter()
        .filter_map(|dir| read_package(dir))
        .collect();

    let scripts = collect_scripts(root, &description_dirs);

    (packages, scripts)
}

/// Walk `root` and return every directory that contains a `DESCRIPTION`
/// file. Honours `.gitignore` / `.ignore` / hidden-file conventions via
/// the `ignore` crate's standard filters.
fn collect_description_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for entry in WalkBuilder::new(root).build().flatten() {
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        if entry.file_name() != "DESCRIPTION" {
            continue;
        }
        if let Some(parent) = entry.path().parent() {
            dirs.push(parent.to_path_buf());
        }
    }
    dirs
}

/// Collect `*.R` files anywhere under `root` that aren't inside any
/// package directory. Files inside `pkg_dir/R/` are owned by that
/// package; files elsewhere in `pkg_dir` (tests/, inst/, etc.) are
/// skipped entirely to avoid double-registering package-internal R
/// sources as workspace scripts.
fn collect_scripts(root: &Path, package_dirs: &[PathBuf]) -> Vec<FileEntry> {
    let mut scripts = Vec::new();
    for entry in WalkBuilder::new(root).build().flatten() {
        let path = entry.path();
        if !is_r_file(path) {
            continue;
        }
        if package_dirs.iter().any(|pkg| path.starts_with(pkg)) {
            continue;
        }
        let Ok(contents) = fs::read_to_string(path) else {
            continue;
        };
        let Ok(url) = UrlId::from_file_path(path) else {
            continue;
        };
        scripts.push(FileEntry { url, contents });
    }
    scripts
}
