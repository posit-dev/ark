//! Goto-definition at the ide layer.
//!
//! These only check that `oak_ide::goto_definition` assembles a
//! `NavigationTarget` from a resolved binding, for a local def and a
//! cross-file `source()` jump. The resolution itself is covered exhaustively
//! by `oak_db`'s `file_resolve_at` / `file_resolve` tests, and the use-def
//! logic by `oak_semantic`; we don't re-test it here.

use biome_rowan::TextSize;
use oak_db::OakDatabase;
use oak_ide::goto_definition;

use crate::support::install_library_package;
use crate::support::range;
use crate::support::upsert;

#[test]
fn test_local_definition_navigates_to_binding() {
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "a.R", "x <- 1\nx\n");

    // Cursor on the use `x` on the second line (offset 7).
    let targets = goto_definition(&db, file, TextSize::from(7u32));
    assert_eq!(targets.len(), 1);
    let target = &targets[0];

    assert_eq!(target.file, file);
    assert_eq!(target.name, "x");
    assert_eq!(target.full_range, range(0, 1));
    assert_eq!(target.focus_range, range(0, 1));
}

#[test]
fn test_navigates_from_trailing_edge_of_identifier() {
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "a.R", "x <- 1\nx\n");

    // Cursor at the trailing edge of the use `x` (offset 8, right after it).
    // `classify` snaps back onto the name token before resolving. A half-open
    // `contains` would otherwise miss the use whose range ends at 8.
    let targets = goto_definition(&db, file, TextSize::from(8u32));
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].name, "x");
    assert_eq!(targets[0].full_range, range(0, 1));
}

#[test]
fn test_navigates_across_source_directive() {
    let mut db = OakDatabase::new();
    let helpers = upsert(&mut db, "helpers.R", "helper <- function() 1\n");
    let script = upsert(&mut db, "script.R", "source(\"helpers.R\")\nhelper\n");

    // Cursor on the forwarded `helper` use, on the line after the source().
    let offset = TextSize::from("source(\"helpers.R\")\n".len() as u32);
    let targets = goto_definition(&db, script, offset);
    assert_eq!(targets.len(), 1);
    let target = &targets[0];

    // The target lives in the sourced file, in that file's coordinates.
    assert_eq!(target.file, helpers);
    assert_eq!(target.name, "helper");
    assert_eq!(target.full_range, range(0, 6));
}

#[test]
fn test_navigates_to_package_export_via_library_call() {
    // The package-layer wiring (`resolve_at` -> `Package::resolve`) must survive
    // the handler's projection to a `NavigationTarget`. db-layer tests assert
    // `def.file`/`def.name`; this pins down that a package-resolved binding
    // actually yields a jump (i.e. its `name_range` is `Some`).
    let mut db = OakDatabase::new();
    let pkg_file =
        install_library_package(&mut db, "mypkg", &["foo"], "a.R", "foo <- function() 42\n");

    // A script attaches `mypkg`, then uses the exported `foo`.
    let script = upsert(&mut db, "script.R", "library(mypkg)\nfoo\n");
    let offset = TextSize::from("library(mypkg)\n".len() as u32);

    let targets = goto_definition(&db, script, offset);
    assert_eq!(targets.len(), 1);
    let target = &targets[0];

    // Jumps into the package file, at the `foo` binding's coordinates.
    assert_eq!(target.file, pkg_file);
    assert_eq!(target.name, "foo");
    assert_eq!(target.full_range, range(0, 3));
}
