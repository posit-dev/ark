use aether_url::UrlId;

use crate::Db;
use crate::File;
use crate::Package;
use crate::PackageOrigin;

/// The collation order for a package: a `Vec<File>` derived from
/// `Package.collation` (the basename spec from `DESCRIPTION`) plus the
/// current set of interned files under the package's `R/` directory.
///
/// Reads `Root.revision(db)` for the package's workspace root so the
/// query invalidates when files are added or removed inside that root.
/// Doesn't depend on `Files` directly because `Files` has no salsa
/// anchor of its own.
///
/// `Some(spec)` (explicit `Collate` field). Resolve each basename
/// against `Files` in spec order, dropping basenames not yet interned.
/// Adding an unrelated `R/` file under the package root doesn't change
/// this result (its basename isn't in the spec), so downstream
/// tracked-query callers get backdated through salsa's PartialEq.
///
/// `None` (alphabetical fallback). Iterate every file in `Files` whose
/// URL is under `<root>/R/` and only contains the basename (no
/// subdirectory), sorted by URL. Adding a new `R/` file does change
/// this list, but only at the position of the new basename.
///
/// Installed packages return an empty `Vec`. Installed-package sources
/// (if available) don't ship through the workspace `R/` directory.
#[salsa::tracked(returns(ref))]
pub fn collation_files(db: &dyn Db, package: Package) -> Vec<File> {
    let PackageOrigin::Workspace { root } = package.kind(db) else {
        return Vec::new();
    };
    // Anchor on the root's revision so the query re-runs when files
    // are added / removed within the package root.
    let _ = root.revision(db);

    let root_url = root.path(db);
    let r_dir = match append_r_segment(root_url) {
        Some(dir) => dir,
        None => {
            log::warn!(
                "Package {} root {} can't be extended to R/",
                package.name(db),
                root_url.as_url(),
            );
            return Vec::new();
        },
    };

    match package.collation(db) {
        Some(spec) => collation_files_from_spec(db, &r_dir, spec),
        None => collation_files_alphabetical(db, &r_dir),
    }
}

fn collation_files_from_spec(db: &dyn Db, r_dir: &UrlId, spec: &[String]) -> Vec<File> {
    spec.iter()
        .filter_map(|basename| {
            let url = append_segment(r_dir, basename)?;
            db.files().get(&url)
        })
        .collect()
}

fn collation_files_alphabetical(db: &dyn Db, r_dir: &UrlId) -> Vec<File> {
    let mut entries: Vec<(UrlId, File)> = db
        .files()
        .entries()
        .into_iter()
        .filter(|(url, _)| is_direct_child(r_dir, url))
        .collect();
    entries.sort_by(|(a, _), (b, _)| a.as_url().as_str().cmp(b.as_url().as_str()));
    entries.into_iter().map(|(_, file)| file).collect()
}

/// Join `<root>` + `R/` into a URL ending with a trailing slash, so
/// `is_direct_child` and `append_segment` can compare prefixes
/// cleanly.
fn append_r_segment(root: &UrlId) -> Option<UrlId> {
    let mut url = root.as_url().clone();
    // `Url::join` resolves relative paths against the existing path. A
    // trailing slash on the root makes "R/" produce "<root>/R/".
    {
        let path = url.path().to_string();
        if !path.ends_with('/') {
            url.set_path(&format!("{path}/"));
        }
    }
    let joined = url.join("R/").ok()?;
    Some(UrlId::from_canonical(joined))
}

fn append_segment(dir: &UrlId, basename: &str) -> Option<UrlId> {
    let joined = dir.as_url().join(basename).ok()?;
    Some(UrlId::from_canonical(joined))
}

/// `true` if `url` sits directly inside `dir` (no subdirectory). Used
/// to filter `Files::entries()` to the package's `R/` content.
fn is_direct_child(dir: &UrlId, url: &UrlId) -> bool {
    let dir_path = dir.as_url().path();
    let url_path = url.as_url().path();
    let Some(rest) = url_path.strip_prefix(dir_path) else {
        return false;
    };
    !rest.is_empty() && !rest.contains('/')
}
