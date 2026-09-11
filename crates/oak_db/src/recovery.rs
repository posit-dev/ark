//! Records every Salsa cycle-recovery handler invoked during a test.
//!
//! Recovery handlers return ordinary fallback values, often an empty `Vec` that
//! a non-cycling query can also return. Tests therefore cannot tell from a
//! query result whether recovery happened. [`record()`] writes the handler and
//! its query key here, allowing tests to assert the handler Salsa chose.
//!
//! The log is process-global so calls from Salsa worker threads are recorded.
//! Nextest runs each test in its own process, so tests do not share this log.

#[cfg(test)]
use std::sync::Mutex;

use crate::file_imports::CollationView;
#[cfg(test)]
use crate::tests::test_db::path_name;
use crate::Db;
use crate::File;
use crate::Name;
use crate::NamespaceVisibility;
use crate::Package;

/// One variant per query that declares a `cycle_result` handler, carrying
/// that query's salsa key.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum Recovery<'db> {
    SemanticIndex(File),
    Exports(File),
    AttachedPackages(File),
    AttachedPackagesAnywhere(File),
    InheritedLayers(File, CollationView),
    CrossFileLayers(File, CollationView),
    PackageResolve(Package, Name<'db>, NamespaceVisibility),
}

#[cfg(test)]
static FIRED: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[cfg(test)]
pub(crate) fn record(db: &dyn Db, recovery: Recovery<'_>) {
    FIRED.lock().unwrap().push(render(db, recovery));
}

#[cfg(not(test))]
pub(crate) fn record(_db: &dyn Db, _recovery: Recovery<'_>) {}

/// Clear before each probe. Salsa memoizes a cycle result, so a warm database
/// does not invoke its handler again. Without a reset, an earlier probe's entry
/// would look like recovery from this probe.
#[cfg(test)]
pub(crate) fn reset() {
    FIRED.lock().unwrap().clear();
}

/// Recorded firings, in the order salsa consulted them.
#[cfg(test)]
pub(crate) fn fired() -> Vec<String> {
    FIRED.lock().unwrap().clone()
}

#[cfg(test)]
fn render(db: &dyn Db, recovery: Recovery<'_>) -> String {
    match recovery {
        Recovery::SemanticIndex(file) => format!("semantic_index({})", path_name(file.path(db))),
        Recovery::Exports(file) => format!("exports({})", path_name(file.path(db))),
        Recovery::AttachedPackages(file) => {
            format!("attached_packages({})", path_name(file.path(db)))
        },
        Recovery::AttachedPackagesAnywhere(file) => {
            format!("attached_packages_anywhere({})", path_name(file.path(db)))
        },
        Recovery::InheritedLayers(file, view) => {
            format!("inherited_layers({}, {view:?})", path_name(file.path(db)))
        },
        Recovery::CrossFileLayers(file, view) => {
            format!("cross_file_layers({}, {view:?})", path_name(file.path(db)))
        },
        Recovery::PackageResolve(package, name, visibility) => format!(
            "Package::resolve({}, {}, {visibility:?})",
            package.name(db),
            name.text(db).as_str(),
        ),
    }
}
