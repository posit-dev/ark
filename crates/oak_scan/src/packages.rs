//! Filesystem-level R package discovery. Pure I/O, no salsa access. Reused by
//! both the library scanner (which walks the package folders of a library
//! directory) and the workspace scanner (which walks the workspace tree looking
//! for `DESCRIPTION` files at any depth).

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use aether_path::FilePath;
use filetime::FileTime;
use ignore::WalkBuilder;
use oak_db::FileRevision;
use oak_package_metadata::description::Description;
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
    /// minting a new one (see `Package::description_path`). The `name`
    /// can't serve as identity because two packages can declare the
    /// same `Package:` field, and dedup picks one of them per root.
    pub description_path: FilePath,
    pub name: String,
    /// Mtime of `DESCRIPTION`, stat'd during the walk. Drives the lazy
    /// `Package::description()` query (and `version` / `collation` on top).
    pub description_revision: FileRevision,
    /// Mtime of the package's `NAMESPACE`, stat'd during the walk. Drives
    /// the lazy `Package::namespace()` query. That query is the only place
    /// `NAMESPACE` gets read and parsed. The walk only stats it.
    pub namespace_revision: FileRevision,
    /// `R/*.R` files: the package's loadable namespace.
    pub files: Vec<FileEntry>,
    /// R files inside the package directory but outside `R/`: tests/,
    /// inst/, vignettes/, data-raw/. They get LSP analysis but aren't
    /// loaded with the package. Empty for library packages (the library
    /// scanner doesn't recurse into pkg_dir for these).
    pub scripts: Vec<FileEntry>,
    pub collation: Option<Vec<String>>,
}

/// Read a package's `DESCRIPTION` for its name and `Collate:` order, and stat
/// `DESCRIPTION` / `NAMESPACE` for their revisions. Returns `None` if
/// `DESCRIPTION` is missing or malformed. The returned entry has empty `files`
/// and `scripts`; callers fill those separately.
///
/// `NAMESPACE` is only stat'd here, never read or parsed. The lazy
/// [`oak_db::Package::namespace`] query does the parse. `DESCRIPTION` is read
/// and parsed because the walk needs the `Package:` name (the workspace
/// directory name isn't authoritative) and the `Collate:` order to sort
/// `R/*.R`. The parsed `collation` only orders this walk's files. It isn't
/// pushed to salsa. [`oak_db::Package::version`] and
/// [`oak_db::Package::collation`] re-read `DESCRIPTION` lazily off the
/// revision.
///
/// Workspace-only: [`read_workspace_package`] wraps this and discovers sources
/// by walking the tree. The library scanner skips it entirely and registers
/// installed packages without reading `DESCRIPTION` (it takes the name from
/// the directory).
pub(crate) fn read_package_metadata(package_dir: &Path) -> Option<PackageEntry> {
    let description_file = package_dir.join("DESCRIPTION");
    let description_text = fs::read_to_string(&description_file).ok()?;
    let description = Description::parse(&description_text).log_err()?;

    let description_revision = file_revision(&description_file);
    let namespace_revision = file_revision(&package_dir.join("NAMESPACE"));
    let description_path = FilePath::from_path_buf(description_file)?;

    let collation = description.collate();

    Some(PackageEntry {
        description_path,
        name: description.name,
        description_revision,
        namespace_revision,
        files: Vec::new(),
        scripts: Vec::new(),
        collation,
    })
}

pub(crate) fn is_r_file(path: &Path) -> bool {
    path.is_file() && oak_core::is_r_file(path)
}

/// Where an R file inside a package directory belongs.
///
/// `R/*.R` (direct children of `R/`) are the package's loadable namespace.
/// Files nested deeper under `R/` are skipped: R loads `R/` as a flat
/// directory, so `R/sub/foo.R` isn't part of the namespace and nothing else
/// reads it. Everything else under the package (tests/, inst/, vignettes/,
/// data-raw/, ...) is a script: analysed but not loaded.
///
/// This is the single definition of the rule. The bulk scanner
/// ([`read_workspace_package()`]) and the file watcher (`crate::watch::classify()`)
/// both route through it so the two can't drift on where a file lands.
#[derive(Debug, PartialEq)]
pub(crate) enum PackagePlacement {
    File,
    Script,
    Skip,
}

pub(crate) fn classify_in_package(package_dir: &Path, path: &Path) -> PackagePlacement {
    let r_dir = package_dir.join("R");
    if path.parent() == Some(r_dir.as_path()) {
        PackagePlacement::File
    } else if path.starts_with(&r_dir) {
        PackagePlacement::Skip
    } else {
        PackagePlacement::Script
    }
}

/// Read just the package name from `package_dir/DESCRIPTION`. Cheaper than
/// [`read_package_metadata`] when the caller only needs to look up an existing
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
        .filter_map(|dir| read_workspace_package(dir).map(|pkg| (dir.clone(), pkg)))
        .collect();

    dedup_packages_by_name(pairs)
}

/// Read a workspace package: metadata plus a single gitignore-aware walk of
/// `package_dir` that classifies every `.R` file through [`classify_in_package`].
///
/// `R/*.R` lands in `files` (sorted by basename, the order R loads a flat `R/`
/// in), everything else under the package lands in `scripts`, and files nested
/// below `R/` are dropped. Honouring `.gitignore` here is what keeps `R/` files
/// and scripts consistent: both come out of the same walk, so a gitignored R
/// file is excluded either way.
fn read_workspace_package(package_dir: &Path) -> Option<PackageEntry> {
    let mut package = read_package_metadata(package_dir)?;

    let mut files: Vec<(PathBuf, FileEntry)> = Vec::new();
    let mut scripts: Vec<FileEntry> = Vec::new();

    for entry in workspace_walker(package_dir).flatten() {
        let path = entry.path();
        if !is_r_file(path) {
            continue;
        }
        let placement = classify_in_package(package_dir, path);
        if placement == PackagePlacement::Skip {
            continue;
        }

        let Some(file_path) = FilePath::from_path_buf(path.to_path_buf()) else {
            log::warn!("Skipping R file, can't build a URL: {}", path.display());
            continue;
        };
        let file = FileEntry {
            path: file_path,
            revision: file_revision(path),
        };

        if placement == PackagePlacement::File {
            files.push((path.to_path_buf(), file));
        } else {
            scripts.push(file);
        }
    }

    // `file_imports()` in `oak_db` reads `package.files` order as the collation
    // chain, so a file only sees the files ordered before it. `Collate:` is R's
    // explicit load order; without it R loads `R/` in case-insensitive
    // alphabetical order. R/ files left out of a `Collate:` directive aren't
    // loaded into the namespace, so they move to `scripts` rather than `files`.
    let (loadable, leftover) = match package.collation.as_deref() {
        Some(order) => order_by_collation(files, order),
        None => order_alphabetically(files),
    };
    scripts.extend(leftover);

    package.files = loadable;
    package.scripts = scripts;

    Some(package)
}

/// Split the package's `R/*.R` files into `(loadable, leftover)` using the
/// `Collate:` directive: `loadable` is the listed files in that order, and
/// `leftover` is the R/ files not listed. Logs mismatches in either direction:
/// `Collate:` entries with no file on disk, and R/ files absent from `Collate:`
/// (which become standalone scripts, see [`read_workspace_package`]).
///
/// TODO(diagnostics): surface these as LSP diagnostics on `DESCRIPTION`
/// instead of just log lines.
fn order_by_collation(
    files: Vec<(PathBuf, FileEntry)>,
    order: &[String],
) -> (Vec<FileEntry>, Vec<FileEntry>) {
    let mut by_name: HashMap<String, (PathBuf, FileEntry)> = HashMap::with_capacity(files.len());
    for (path, file) in files {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            log::warn!("Skipping R file with non-UTF8 basename: {}", path.display());
            continue;
        };
        by_name.insert(name.to_string(), (path, file));
    }

    let mut loadable = Vec::with_capacity(order.len());
    for name in order {
        match by_name.remove(name) {
            Some((_, file)) => loadable.push(file),
            None => {
                log::warn!("`Collate:` lists `{name}` but no matching file is present under `R/`")
            },
        }
    }

    // Anything left in `by_name` is on disk but not in `Collate:`. R won't load
    // it into the namespace, so it can't go in `files`; keep it as a standalone
    // script instead of dropping it. Sorted for a deterministic order.
    let mut leftover: Vec<(PathBuf, FileEntry)> = by_name.into_values().collect();
    leftover.sort_by_key(|(path, _)| basename_key(path));
    for (path, _) in &leftover {
        log::warn!(
            "R file `{}` is not listed in `Collate:`; treating it as a standalone \
             script (R will not load it into the namespace)",
            path.display(),
        );
    }

    (
        loadable,
        leftover.into_iter().map(|(_, file)| file).collect(),
    )
}

/// All files are loadable, in case-insensitive alphabetical order by basename.
/// No leftover: without `Collate:`, R loads every R/ file.
fn order_alphabetically(mut files: Vec<(PathBuf, FileEntry)>) -> (Vec<FileEntry>, Vec<FileEntry>) {
    files.sort_by_key(|(path, _)| basename_key(path));
    (
        files.into_iter().map(|(_, file)| file).collect(),
        Vec::new(),
    )
}

/// Case-insensitive sort key from a path's basename.
fn basename_key(path: &Path) -> Option<std::ffi::OsString> {
    path.file_name().map(|name| name.to_ascii_lowercase())
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
        let Some(file_path) = FilePath::from_path_buf(path.to_path_buf()) else {
            continue;
        };
        scripts.push(FileEntry {
            path: file_path,
            revision: file_revision(path),
        });
    }
    scripts
}

/// The file's last-modification time as a [`FileRevision`], or
/// [`FileRevision::zero`] if the metadata can't be read. A zero revision means
/// the next `source_text` still reads disk, it just doesn't get re-invalidated
/// until a real mtime lands.
///
/// We go through `filetime::FileTime::from_last_modification_time` rather than
/// `std::fs::Metadata::modified` to match ty: `FileTime` carries signed
/// seconds, so a pre-epoch mtime stays distinct instead of collapsing onto the
/// `zero()` sentinel the way a `SystemTime` before `UNIX_EPOCH` would.
pub(crate) fn file_revision(path: &Path) -> FileRevision {
    fs::metadata(path)
        .map(|metadata| FileRevision::from(FileTime::from_last_modification_time(&metadata)))
        .unwrap_or_else(|_| FileRevision::zero())
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::classify_in_package;
    use super::PackagePlacement;

    #[test]
    fn classify_in_package_rule() {
        let pkg = Path::new("/ws/pkg");

        // Direct children of `R/` are the loadable namespace.
        assert_eq!(
            classify_in_package(pkg, Path::new("/ws/pkg/R/a.R")),
            PackagePlacement::File
        );

        // Nested under `R/` is excluded: R loads `R/` flat.
        assert_eq!(
            classify_in_package(pkg, Path::new("/ws/pkg/R/sub/b.R")),
            PackagePlacement::Skip
        );

        // Everything else under the package is a script.
        assert_eq!(
            classify_in_package(pkg, Path::new("/ws/pkg/tests/testthat/test-a.R")),
            PackagePlacement::Script
        );
        assert_eq!(
            classify_in_package(pkg, Path::new("/ws/pkg/inst/foo.R")),
            PackagePlacement::Script
        );
        assert_eq!(
            classify_in_package(pkg, Path::new("/ws/pkg/data-raw/prep.R")),
            PackagePlacement::Script
        );
    }
}
