//! Regression test against the real, on-disk base R source cache.
//!
//! `Package::resolve()` has no `NAMESPACE` to gate a base-priority package
//! (see `crates/oak_db/src/package_resolve.rs`), so resolving any bare name
//! against one sweeps every file in it and semantically indexes each one.
//! `File::resolve()` walks the default search path in priority order
//! (`stats, graphics, grDevices, utils, datasets, methods, base`, see
//! `crates/oak_db/src/search.rs`) and stops at the first package that binds
//! the name, but only after that package's full sweep completes. `bquote`
//! only lives in `base`, the *last* layer, so resolving it sweeps all six
//! other default packages first.
//!
//! The other `oak_ide` tests install tiny synthetic packages that can't
//! exercise that walk at the scale it actually runs at. This test points at
//! every default-search-path package's real source tree instead, fetched
//! through the same `oak_source` cache the kernel warms in the background,
//! so a regression that makes the walk much more expensive shows up here as
//! a slow test rather than only as a slow LSP request.
//!
//! The first run needs network access to populate the shared cache under
//! `<cache_dir>/oak/source/v1/r/`. Later runs reuse it.
//!
//! `test_bquote_hole_reference_against_real_search_path` is `#[ignore]`d: it
//! reproduces an open bug (the walk hangs well past a minute rather than
//! finishing in the few seconds file count alone would predict) rather than
//! asserting a fix, so it can't run unattended in normal `just test`.

use std::fs;
use std::path::Path;

use oak_db::Db;
use oak_db::OakDatabase;
use oak_scan::DbScan;
use oak_source::SourceCache;

use crate::support::offset;
use crate::support::range;
use crate::support::ranges;
use crate::support::upsert;

const R_VERSION: &str = "4.5.2";

/// R's default search path, `stats` (highest priority) through `base`
/// (lowest), see `crate::search::DEFAULT_SEARCH_PATH_PACKAGES` in `oak_db`.
const DEFAULT_SEARCH_PATH_PACKAGES: [&str; 7] =
    ["stats", "graphics", "grDevices", "utils", "datasets", "methods", "base"];

/// Register every default-search-path package as a library package backed by
/// its real source directory under `r_root`.
fn install_default_search_path(db: &mut OakDatabase, r_root: &Path) {
    let lib = tempfile::tempdir().unwrap();
    for name in DEFAULT_SEARCH_PATH_PACKAGES {
        fs::create_dir_all(lib.path().join(name)).unwrap();
        fs::write(
            lib.path().join(name).join("DESCRIPTION"),
            format!("Package: {name}\nVersion: {R_VERSION}\n"),
        )
        .unwrap();
    }
    // `set_library_paths` only reads `lib` once, up front, to discover
    // package directories, so it's fine to drop the `TempDir` handle (and
    // its auto-cleanup) right after.
    db.set_library_paths(&[lib.keep()]);

    for name in DEFAULT_SEARCH_PATH_PACKAGES {
        let pkg = db.package_by_name(name).unwrap();
        db.set_package_sources(pkg, &r_root.join(name).join("R"));
    }
}

#[test]
#[ignore]
fn test_bquote_hole_reference_against_real_search_path() {
    let source = SourceCache::open().unwrap();
    let r_root = source
        .get_r(R_VERSION)
        .or_else(|| source.insert_r(R_VERSION))
        .expect("base R source archive unavailable");

    let mut db = OakDatabase::new();
    install_default_search_path(&mut db, &r_root);

    // Same repro as `find_references::test_bquote_hole_nested_in_quoted_call`,
    // but `bquote` now resolves through the real default search path instead
    // of no package at all, so this forces the full walk-and-sweep.
    let script = "foo <- function() {}\nbquote(foo(.(foo)))\n";
    let file = upsert(&mut db, "test.R", script);

    let hole_use = script.rfind("foo").unwrap() as u32;
    let refs = oak_ide::find_references(&db, file, offset(0), true);
    assert_eq!(ranges(&refs), vec![
        range(0, 3),
        range(hole_use, hole_use + 3)
    ]);

    // `bquote` itself isn't locally scoped, so it resolves through the
    // default search path, sweeping `stats`, `graphics`, `grDevices`,
    // `utils`, `datasets`, and `methods` (all empty) before reaching `base`.
    // Its own (library, excluded) definition site doesn't come back, just
    // the call.
    let bquote_offset = script.find("bquote").unwrap() as u32;
    let refs = oak_ide::find_references(&db, file, offset(bquote_offset), true);
    assert_eq!(ranges(&refs), vec![range(bquote_offset, bquote_offset + 6)]);
}
