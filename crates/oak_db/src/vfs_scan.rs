//! Workspace scanner. Walks workspace roots and produces a [`ScanResult`]
//! of R packages (discovered via `DESCRIPTION` files) and stand-alone R
//! scripts. Pure I/O, no salsa access. The result is applied to the
//! database by [`super::Vfs::apply_scan`].
//!
//! Uses the `ignore` crate so `.gitignore` is honoured. The skip set
//! ([`SKIP_DIRS`]) mirrors r-a / ty conventions for editor workspaces.

use std::path::Path;
use std::path::PathBuf;

use ignore::WalkBuilder;
use oak_package_metadata::description::Description;
use oak_package_metadata::namespace::Namespace;

/// Result of scanning one or more workspace roots.
#[derive(Debug, Default)]
pub struct ScanResult {
    pub packages: Vec<PackageDescriptor>,
    /// R script files that aren't inside any package's `R/` directory.
    pub scripts: Vec<FileDescriptor>,
}

/// A workspace package discovered via its `DESCRIPTION` file.
#[derive(Debug)]
pub struct PackageDescriptor {
    pub root: PathBuf,
    pub name: String,
    pub namespace: Namespace,
    /// `R/` file contents in load order. The order is derived from
    /// `DESCRIPTION`'s `Collate` (if set) or the alphabetical list of
    /// `R/` basenames.
    pub files: Vec<FileDescriptor>,
    /// The `Collate` field's value if `DESCRIPTION` set it. `None` means
    /// load order is alphabetical. Persisted into `Package.collation`
    /// so `collation_files` can re-derive the materialised `Vec<File>`
    /// without `apply_scan` doing it eagerly.
    pub collation_spec: Option<Vec<String>>,
}

#[derive(Debug)]
pub struct FileDescriptor {
    pub path: PathBuf,
    pub contents: String,
}

/// Walk `roots`, returning all discovered packages and stand-alone scripts.
///
/// Reads file contents synchronously so the result is self-contained.
/// Callers can apply it on a different thread / loop tick without further
/// I/O.
pub fn scan(roots: &[PathBuf]) -> ScanResult {
    let mut result = ScanResult::default();
    let mut package_roots: Vec<PathBuf> = Vec::new();

    for root in roots {
        for description_path in walk_descriptions(root) {
            let Some(package_root) = description_path.parent() else {
                continue;
            };
            let Some(descriptor) = load_package(package_root) else {
                continue;
            };
            package_roots.push(package_root.to_path_buf());
            result.packages.push(descriptor);
        }
    }

    for root in roots {
        for r_file_path in walk_r_files(root) {
            if package_roots.iter().any(|pkg| r_file_path.starts_with(pkg)) {
                continue;
            }
            if let Some(file) = load_file(&r_file_path) {
                result.scripts.push(file);
            }
        }
    }

    result
}

fn load_package(package_root: &Path) -> Option<PackageDescriptor> {
    let description_path = package_root.join("DESCRIPTION");
    let description_text = match std::fs::read_to_string(&description_path) {
        Ok(text) => text,
        Err(err) => {
            log::warn!(
                "Can't read {path}: {err}",
                path = description_path.display()
            );
            return None;
        },
    };
    let description = match Description::parse(&description_text) {
        Ok(d) => d,
        Err(err) => {
            log::warn!(
                "Can't parse {path}: {err:?}",
                path = description_path.display()
            );
            return None;
        },
    };

    let namespace = load_namespace(package_root).unwrap_or_default();

    let r_dir = package_root.join("R");
    let collation_spec = description.collate();
    let ordered_paths: Vec<PathBuf> = match &collation_spec {
        Some(names) => names.iter().map(|n| r_dir.join(n)).collect(),
        None => {
            let mut paths = list_r_files(&r_dir);
            paths.sort();
            paths
        },
    };

    let files: Vec<FileDescriptor> = ordered_paths
        .into_iter()
        .filter_map(|p| load_file(&p))
        .collect();

    Some(PackageDescriptor {
        root: package_root.to_path_buf(),
        name: description.name,
        namespace,
        files,
        collation_spec,
    })
}

fn load_namespace(package_root: &Path) -> Option<Namespace> {
    let path = package_root.join("NAMESPACE");
    if !path.is_file() {
        return None;
    }
    let contents = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(err) => {
            log::warn!("Can't read {path}: {err}", path = path.display());
            return None;
        },
    };
    match Namespace::parse(&contents) {
        Ok(ns) => Some(ns),
        Err(err) => {
            log::warn!("Can't parse {path}: {err:?}", path = path.display());
            None
        },
    }
}

fn load_file(path: &Path) -> Option<FileDescriptor> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Some(FileDescriptor {
            path: path.to_path_buf(),
            contents,
        }),
        Err(err) => {
            log::warn!("Can't read {path}: {err}", path = path.display());
            None
        },
    }
}

fn list_r_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| is_r_file(p))
        .collect()
}

fn is_r_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("r"))
}

fn walk_descriptions(root: &Path) -> Vec<PathBuf> {
    build_walker(root)
        .filter(|entry| entry.file_name() == "DESCRIPTION")
        .map(|entry| entry.into_path())
        .collect()
}

fn walk_r_files(root: &Path) -> Vec<PathBuf> {
    build_walker(root)
        .filter(|entry| is_r_file(entry.path()))
        .map(|entry| entry.into_path())
        .collect()
}

fn build_walker(root: &Path) -> impl Iterator<Item = ignore::DirEntry> {
    WalkBuilder::new(root)
        .filter_entry(|entry| {
            entry
                .file_name()
                .to_str()
                .map(|name| !SKIP_DIRS.contains(&name))
                .unwrap_or(true)
        })
        .build()
        .filter_map(|res| res.ok())
        .filter(|entry| entry.file_type().is_some_and(|ft| ft.is_file()))
}

const SKIP_DIRS: &[&str] = &[
    ".git",
    ".Rproj.user",
    "renv",
    "packrat",
    ".Rcheck",
    "node_modules",
    ".quarto",
];
