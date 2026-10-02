use std::borrow::Cow;

use camino::Utf8Path;

use crate::file_imports::CollationView;
use crate::load_context::load_environment;
use crate::load_context::EnvironmentChain;
use crate::load_context::LoadContext;
use crate::load_context::LoadEnvironment;
use crate::load_context::LoadKind;
use crate::load_context::LoaderInfo;
use crate::File;
use crate::SourceDb;

const LOADER: LoaderInfo = LoaderInfo {
    name: "testthat",
    loads: "`helper*.R` and `setup*.R` before your tests",
};

/// A `tests/testthat/` file. It runs with the package loaded and `testthat`
/// attached, after testthat has sourced the package's `helper*.R` and
/// `setup*.R` files into the test environment.
///
/// Support files form their own collation. An `Eager` view keeps only
/// source-order predecessors, while a `Deferred` view keeps every support file.
/// Every `R/` file remains visible because package loading finishes first.
pub(crate) fn load_context(
    db: &dyn SourceDb,
    file: File,
    view: CollationView,
) -> Option<LoadContext> {
    let package = file.package(db)?;
    if !is_testthat_file(file, db) {
        return None;
    }

    let mut support: Vec<File> = package
        .scripts(db)
        .iter()
        .copied()
        .filter(|script| is_testthat_support_file(*script, db))
        .collect();
    support.sort_by_cached_key(|script| testthat_support_key(*script, db));

    let mut environments = match support.iter().position(|script| *script == file) {
        Some(position) => EnvironmentChain(vec![load_environment(file, &support, view, position)]),
        None => {
            let mut chain = EnvironmentChain::own(file);
            chain.0.push(LoadEnvironment { files: support });
            chain
        },
    };

    // The test environment shadows the package namespace regardless of load
    // order. Within the namespace, reverse collation order gives later files
    // priority.
    environments.0.push(LoadEnvironment {
        files: package.files(db).to_vec(),
    });

    Some(LoadContext {
        kind: LoadKind::Namespace(package),
        environments,
        implicit_attaches: vec!["testthat"],
        loader: Some(LOADER),
    })
}

/// True when `file` sits directly in a `tests/testthat/` directory, the
/// layout testthat sources and runs files from. This is what separates a
/// test file from an ordinary package script under e.g. `tests/` or `inst/`.
fn is_testthat_file(file: File, db: &dyn SourceDb) -> bool {
    match file.path(db).as_file() {
        Some(path) => in_testthat_dir(path.as_path()),
        None => false,
    }
}

fn in_testthat_dir(path: &Utf8Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    parent.file_name() == Some("testthat") &&
        parent.parent().and_then(Utf8Path::file_name) == Some("tests")
}

/// `testthat` loads `helper*.R` and `setup*.R` before tests, so their bindings
/// are visible. Teardown files run afterward and are excluded.
fn is_testthat_support_file(file: File, db: &dyn SourceDb) -> bool {
    if !is_testthat_file(file, db) {
        return false;
    }
    match file.path(db).file_name() {
        Some(name) => name.starts_with("helper") || name.starts_with("setup"),
        None => false,
    }
}

/// Byte-wise basename sort key keeps support-file precedence platform-stable.
fn testthat_support_key(file: File, db: &dyn SourceDb) -> Cow<'_, str> {
    file.path(db).file_name().unwrap_or_default()
}
