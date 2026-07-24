//! Regression test against the real, on-disk base R source cache.
//!
//! `Package::resolve()` has no `NAMESPACE` to gate `base` (see
//! `crates/oak_db/src/package_resolve.rs`), so resolving any bare name
//! against it sweeps every file in the package (around 160) and semantically
//! indexes each one. The other `oak_ide` tests install tiny synthetic
//! packages that can't exercise that sweep at the scale it actually runs at.
//! This test points at base's real source tree instead, fetched through the
//! same `oak_source` cache the kernel warms in the background, so a
//! regression that makes per-file indexing much more expensive shows up here
//! as a slow test rather than only as a slow LSP request.
//!
//! Ignored by default: the first run needs network access to populate the
//! shared cache under `<cache_dir>/oak/source/v1/r/`. Later runs reuse it, so
//! run it explicitly with `cargo test -- --ignored` (or via `just test
//! --run-ignored all`) once that cache is warm.

use std::fs;

use oak_db::Db;
use oak_db::OakDatabase;
use oak_scan::DbScan;
use oak_source::SourceCache;

use crate::support::offset;
use crate::support::range;
use crate::support::ranges;
use crate::support::upsert;

const R_VERSION: &str = "4.5.2";

#[test]
fn test_bquote_hole_reference_against_real_base() {
    let source = SourceCache::open().unwrap();
    let r_root = source
        .get_r(R_VERSION)
        .or_else(|| source.insert_r(R_VERSION))
        .expect("base R source archive unavailable");
    let base_r_dir = r_root.join("base").join("R");

    let lib = tempfile::tempdir().unwrap();
    fs::create_dir_all(lib.path().join("base")).unwrap();
    fs::write(
        lib.path().join("base").join("DESCRIPTION"),
        format!("Package: base\nVersion: {R_VERSION}\n"),
    )
    .unwrap();

    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    let base = db.package_by_name("base").unwrap();
    db.set_package_sources(base, &base_r_dir);

    // Same repro as `find_references::test_bquote_hole_nested_in_quoted_call`,
    // but `bquote` now resolves through the real `base` package instead of no
    // package at all, so this forces the full-package sweep.
    let script = "foo <- function() {}\nbquote(foo(.(foo)))\n";
    let file = upsert(&mut db, "test.R", script);

    let hole_use = script.rfind("foo").unwrap() as u32;
    let refs = oak_ide::find_references(&db, file, offset(0), true);
    assert_eq!(ranges(&refs), vec![
        range(0, 3),
        range(hole_use, hole_use + 3)
    ]);

    // `bquote` itself isn't locally scoped, so it resolves through the
    // `base` import layer and forces `Package::resolve()` to sweep every
    // file in the package looking for a top-level `bquote` binding. Its own
    // (library, excluded) definition site doesn't come back, just the call.
    let bquote_offset = script.find("bquote").unwrap() as u32;
    let refs = oak_ide::find_references(&db, file, offset(bquote_offset), true);
    assert_eq!(ranges(&refs), vec![range(bquote_offset, bquote_offset + 6)]);
}
