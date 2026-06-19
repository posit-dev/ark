//! Shared test helpers for the oak_ide integration suite.

use aether_path::FilePath;
use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_db::DbInputs;
use oak_db::File;
use oak_db::FileRevision;
use oak_db::OakDatabase;
use oak_db::Package;
use oak_db::Root;
use oak_db::RootKind;
use oak_ide::FileRange;
use oak_package_metadata::namespace::Namespace;
use oak_scan::DbScan;
use salsa::Setter;
use stdext::SortedVec;
use url::Url;

pub fn file_url(name: &str) -> Url {
    // `Url::to_file_path` on Windows requires a drive-letter prefix, so
    // synthesize one for tests. Linux is happy with rootless paths.
    if cfg!(windows) {
        Url::parse(&format!("file:///C:/project/R/{name}")).unwrap()
    } else {
        Url::parse(&format!("file:///project/R/{name}")).unwrap()
    }
}

pub fn lib_url(name: &str) -> Url {
    Url::parse(&format!("file:///library/{name}")).unwrap()
}

pub fn workspace_url(name: &str) -> Url {
    Url::parse(&format!("file:///workspace/{name}")).unwrap()
}

pub fn upsert(db: &mut OakDatabase, name: &str, contents: &str) -> File {
    db.upsert_editor(FilePath::from_url(&file_url(name)), contents.to_string())
}

pub fn offset(n: u32) -> TextSize {
    TextSize::from(n)
}

pub fn range(start: u32, end: u32) -> TextRange {
    TextRange::new(TextSize::from(start), TextSize::from(end))
}

/// Project results to in-file ranges (single-file tests).
pub fn ranges(refs: &[FileRange]) -> Vec<TextRange> {
    refs.iter().map(|r| r.range).collect()
}

/// Project results to `(file, range)` pairs (cross-file tests).
pub fn pairs(refs: &[FileRange]) -> Vec<(File, TextRange)> {
    refs.iter().map(|r| (r.file, r.range)).collect()
}

/// Install `name` as a library package exporting `exports`, with one file at
/// `R/{file_name}`. Returns the package file.
pub fn install_library_package(
    db: &mut OakDatabase,
    name: &str,
    exports: &[&str],
    file_name: &str,
    contents: &str,
) -> File {
    install_pkg(db, RootKind::Library, name, exports, file_name, contents)
}

/// Install `name` as a workspace package exporting `exports`, with one file at
/// `R/{file_name}`. Returns the package file.
pub fn install_workspace_package(
    db: &mut OakDatabase,
    name: &str,
    exports: &[&str],
    file_name: &str,
    contents: &str,
) -> File {
    install_pkg(db, RootKind::Workspace, name, exports, file_name, contents)
}

fn install_pkg(
    db: &mut OakDatabase,
    kind: RootKind,
    name: &str,
    exports: &[&str],
    file_name: &str,
    contents: &str,
) -> File {
    let (pkg_url, file_url, root_url) = match kind {
        RootKind::Library => (
            lib_url(&format!("{name}/DESCRIPTION")),
            lib_url(&format!("{name}/R/{file_name}")),
            lib_url(name),
        ),
        RootKind::Workspace => (
            workspace_url(&format!("{name}/DESCRIPTION")),
            workspace_url(&format!("{name}/R/{file_name}")),
            workspace_url(name),
        ),
    };
    let namespace = Namespace {
        exports: SortedVec::from_vec(exports.iter().map(|s| s.to_string()).collect()),
        ..Default::default()
    };
    let pkg = Package::new(
        db,
        FilePath::from_url(&pkg_url),
        name.to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        Some(namespace),
        Vec::new(),
        Vec::new(),
    );
    let file = File::new(
        db,
        FilePath::from_url(&file_url),
        FileRevision::zero(),
        Some(contents.to_string()),
        Some(pkg),
    );
    pkg.set_files(db).to(vec![file]);
    let root = Root::new(db, FilePath::from_url(&root_url), kind, Vec::new(), vec![
        pkg,
    ]);
    match kind {
        // Append rather than replace, so a test can install several library
        // packages into the database.
        RootKind::Library => {
            let mut roots = db.library_roots().roots(db).clone();
            roots.push(root);
            db.library_roots().set_roots(db).to(roots);
        },
        RootKind::Workspace => {
            db.workspace_roots().set_roots(db).to(vec![root]);
        },
    };
    file
}
