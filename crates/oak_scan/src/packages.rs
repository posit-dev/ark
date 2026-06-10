//! Filesystem-level R package discovery. Pure I/O, no salsa access. Reused by
//! both the library scanner (which walks the package folders of a library
//! directory) and the workspace scanner (which walks the workspace tree looking
//! for `DESCRIPTION` files at any depth).

use std::collections::HashMap;
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
/// plus the R files under `R/`, plus any package-internal R files in
/// `tests/`, `inst/`, etc. (populated only by the workspace scanner).
#[derive(Debug)]
pub(crate) struct PackageEntry {
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
    /// `R/*.R` files: the package's loadable namespace.
    pub files: Vec<FileEntry>,
    /// R files inside the package directory but outside `R/`: tests/,
    /// inst/, vignettes/, data-raw/. They get LSP analysis but aren't
    /// loaded with the package. Empty for library packages (the library
    /// scanner doesn't recurse into pkg_dir for these).
    pub scripts: Vec<FileEntry>,
    pub collation: Option<Vec<String>>,
}

/// Read a candidate package directory. Returns `None` if `DESCRIPTION`
/// is missing or malformed. Populates `files` (R/*.R) only; `scripts`
/// is filled by [`scan_workspace_packages`] for workspace packages.
pub(crate) fn read_package(package_dir: &Path) -> Option<PackageEntry> {
    let description_path = package_dir.join("DESCRIPTION");
    let description_text = fs::read_to_string(&description_path).ok()?;
    let description = Description::parse(&description_text).log_err()?;
    let description_url = UrlId::from_file_path(&description_path).ok()?;

    let namespace = fs::read_to_string(package_dir.join("NAMESPACE"))
        .ok()
        .and_then(|text| Namespace::parse(&text).log_err())
        .unwrap_or_default();

    let files = scan_r_files(&package_dir.join("R"));
    let collation = description.collate();

    Some(PackageEntry {
        description_url,
        name: description.name,
        version: Some(description.version),
        namespace,
        files,
        scripts: Vec::new(),
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

pub(crate) fn is_r_file(path: &Path) -> bool {
    path.is_file() && oak_core::is_r_file(path)
}

/// Read just the package name from `package_dir/DESCRIPTION`. Cheaper than
/// [`read_package`] when the caller only needs to look up an existing
/// `Package` by name.
pub(crate) fn read_description_name(package_dir: &Path) -> Option<String> {
    let text = fs::read_to_string(package_dir.join("DESCRIPTION")).ok()?;
    Description::parse(&text).log_err().map(|d| d.name)
}

/// Walk a workspace root for its packages: every directory that contains a
/// `DESCRIPTION`, at any depth.
///
/// For each package:
/// - `pkg.files` is `{pkg_dir}/R/*.R` (the loadable namespace).
/// - `pkg.scripts` is every other `.R` file under `pkg_dir/` (tests/,
///   inst/, vignettes/, data-raw/, etc.).
///
/// If two `DESCRIPTION` files in the workspace declare the same `Package:`
/// name, the one whose directory sorts first wins and the rest are dropped with
/// a warn log. See [`dedup_packages_by_name`] for the rationale.
pub(crate) fn scan_workspace_packages(root: &Path) -> Vec<PackageEntry> {
    let mut description_dirs = collect_description_dirs(root);
    description_dirs.sort();

    let pairs: Vec<(PathBuf, PackageEntry)> = description_dirs
        .iter()
        .filter_map(|dir| {
            read_package(dir).map(|mut pkg| {
                pkg.scripts = scan_package_scripts(dir);
                (dir.clone(), pkg)
            })
        })
        .collect();

    dedup_packages_by_name(pairs)
}

/// Walk a workspace root for its top-level scripts: every `.R` file that isn't
/// inside a package directory.
///
/// Package-internal files belong to their package (`R/*.R` as `files`, the rest
/// as `scripts`), so we skip any `.R` file under a directory that has a
/// `DESCRIPTION`. We re-derive those directories here rather than taking them
/// from [`scan_workspace_packages`]: that keeps the two scans independent, and
/// it excludes the losing side of a same-name package duplicate too. That loser
/// is dropped by `scan_workspace_packages`, but its files are still package
/// internals, not loose scripts. The walk is filename-only, so it's cheap next
/// to reading every script's contents.
pub(crate) fn scan_workspace_scripts(root: &Path) -> Vec<FileEntry> {
    let package_dirs = collect_description_dirs(root);
    collect_scripts(root, &package_dirs)
}

/// Walk a package directory, returning every `.R` file inside it that's
/// not in `pkg_dir/R/`. R/ files are owned by [`PackageEntry::files`];
/// everything else (tests/, inst/, vignettes/, data-raw/) lands here.
fn scan_package_scripts(pkg_dir: &Path) -> Vec<FileEntry> {
    let mut scripts = Vec::new();
    let r_dir = pkg_dir.join("R");
    for entry in workspace_walker(pkg_dir).flatten() {
        let path = entry.path();
        if !is_r_file(path) {
            continue;
        }
        if path.starts_with(&r_dir) {
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

/// Keep the first occurrence of each `Package:` name, dropping duplicates with
/// a warn log naming both directories.
///
/// `Package` identity in oak is keyed on `(root, name)`, so two same-name
/// DESCRIPTIONs in one workspace would otherwise collapse into one `Package`
/// entity with the loser's files clobbering the winner's and the duplicate
/// appearing twice in `root.packages`. First-wins gives a stable, predictable
/// outcome at the cost of ignoring one of the two.
fn dedup_packages_by_name(pairs: Vec<(PathBuf, PackageEntry)>) -> Vec<PackageEntry> {
    let mut winners: HashMap<String, PathBuf> = HashMap::new();
    let mut packages: Vec<PackageEntry> = Vec::with_capacity(pairs.len());

    for (dir, pkg) in pairs {
        if let Some(existing) = winners.get(&pkg.name) {
            log::warn!(
                "Duplicate package name `{name}` in workspace: keeping {first}, skipping {dup}",
                name = pkg.name,
                first = existing.display(),
                dup = dir.display(),
            );
            continue;
        }
        winners.insert(pkg.name.clone(), dir);
        packages.push(pkg);
    }

    packages
}

/// Walk `root` and return every directory that contains a `DESCRIPTION`
/// file.
fn collect_description_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for entry in workspace_walker(root).flatten() {
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

/// Collect `*.R` files anywhere under `root` that aren't inside any package
/// directory. Files inside `pkg_dir/R/` are owned by that package; files
/// elsewhere in `pkg_dir` (tests/, inst/, etc.) are skipped entirely to avoid
/// double-registering package-internal R sources as workspace scripts.
fn collect_scripts(root: &Path, package_dirs: &[PathBuf]) -> Vec<FileEntry> {
    let mut scripts = Vec::new();
    for entry in workspace_walker(root).flatten() {
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

/// Build a directory walker for workspace discovery.
///
/// Uses the `ignore` crate's default `WalkBuilder`, which honours `.gitignore`
/// / `.ignore` and skips hidden directories. Matches the ty / Astral / ruff
/// convention. The concrete payoff for R is `renv/library/` exclusion. Renv
/// snapshots each installed package with its own `DESCRIPTION` and `R/`, so
/// walking into them would surface vendored packages and vendored R files as
/// workspace content.
///
/// rust-analyzer takes the opposite approach: walk everything, hardcoded
/// exclusions for `.git` and `target`. We may want to revisit if the implicit
/// `.gitignore` filtering surprises users, in which case the natural next step
/// is an opt-out config (similar to ty's `respect-ignore-files`) and / or a
/// hardcoded exclusion list.
fn workspace_walker(root: &Path) -> ignore::Walk {
    WalkBuilder::new(root).build()
}
