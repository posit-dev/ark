//! Goto-definition at the ide layer.
//!
//! These only check that `oak_ide::goto_definition()` assembles a
//! `NavigationTarget` from a resolved binding, i.e.:
//!
//! - A local def
//! - A cross-file `source()` jump
//! - A `pkg::sym` / `pkg:::sym` namespace access
//!
//! The resolution itself is covered exhaustively by `oak_db`'s `file_resolve_at()` /
//! `file_resolve()` / `package_resolve()` tests, and the use-def logic by `oak_semantic`,
//! we don't re-test it here.

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

#[test]
fn test_navigates_via_namespace_access_to_exported_binding() {
    let mut db = OakDatabase::new();

    let pkg_file =
        install_library_package(&mut db, "mypkg", &["foo"], "a.R", "foo <- function() 42\n");

    let script = upsert(&mut db, "script.R", "mypkg::foo\n");

    // Cursor on `mypkg::f<@>oo`
    let targets = goto_definition(&db, script, TextSize::from(8));
    assert_eq!(targets.len(), 1);

    let target = &targets[0];
    assert_eq!(target.file, pkg_file);
    assert_eq!(target.name, "foo");
    assert_eq!(target.full_range, range(0, 3));
}

#[test]
fn test_navigates_via_namespace_access_to_internal_binding() {
    let mut db = OakDatabase::new();

    let pkg_file = install_library_package(&mut db, "mypkg", &[], "a.R", "foo <- function() 42\n");

    let script = upsert(&mut db, "script.R", "mypkg:::foo\n");

    // Cursor on `mypkg:::f<@>oo`
    let targets = goto_definition(&db, script, TextSize::from(9));
    assert_eq!(targets.len(), 1);

    let target = &targets[0];
    assert_eq!(target.file, pkg_file);
    assert_eq!(target.name, "foo");
    assert_eq!(target.full_range, range(0, 3));
}

#[test]
fn test_namespace_access_cant_jump_to_internal_binding_with_colon_colon() {
    // i.e. can't jump to internal `foo()` with `mypkg::foo()`!
    let mut db = OakDatabase::new();

    install_library_package(&mut db, "mypkg", &[], "a.R", "foo <- function() 42\n");

    let script = upsert(&mut db, "script.R", "mypkg::foo\n");

    let targets = goto_definition(&db, script, TextSize::from(8));
    assert!(targets.is_empty());
}

#[test]
fn test_namespace_access_can_jump_to_exported_binding_with_colon_colon_colon() {
    // i.e. can jump to exported `foo()` with `mypkg:::foo()`!
    let mut db = OakDatabase::new();

    let pkg_file =
        install_library_package(&mut db, "mypkg", &["foo"], "a.R", "foo <- function() 42\n");

    let script = upsert(&mut db, "script.R", "mypkg:::foo\n");

    // Cursor on `mypkg:::f<@>oo`
    let targets = goto_definition(&db, script, TextSize::from(9));
    assert_eq!(targets.len(), 1);

    let target = &targets[0];
    assert_eq!(target.file, pkg_file);
    assert_eq!(target.name, "foo");
    assert_eq!(target.full_range, range(0, 3));
}

#[test]
fn test_namespace_access_to_unknown_package() {
    // No package named `mypkg` is installed, so the access resolves to nothing.
    let mut db = OakDatabase::new();
    let script = upsert(&mut db, "script.R", "mypkg::foo\n");
    let targets = goto_definition(&db, script, TextSize::from(8));
    assert!(targets.is_empty());
}

#[test]
fn test_navigates_to_both_candidates_through_source() {
    // The sourced file binds `foo` on both arms of a top-level `if`/`else`, so
    // the name has two candidate definitions. Multi-target exports carry both
    // through the `source()` forward, and goto-def offers both jumps.
    let mut db = OakDatabase::new();
    let helpers = upsert(&mut db, "helpers.R", "if (cond) foo <- 1 else foo <- 2\n");
    let script = upsert(&mut db, "script.R", "source(\"helpers.R\")\nfoo\n");

    let offset = TextSize::from("source(\"helpers.R\")\n".len() as u32);
    let mut targets = goto_definition(&db, script, offset);
    assert_eq!(targets.len(), 2);

    // Both land in the sourced file, in definition order.
    targets.sort_by_key(|t| t.focus_range.start());
    assert!(targets.iter().all(|t| t.file == helpers && t.name == "foo"));
    assert_eq!(targets[0].focus_range, range(10, 13));
    assert_eq!(targets[1].focus_range, range(24, 27));
}

#[test]
fn test_navigates_to_assign_binding() {
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "a.R", "assign(\"x\", 1)\nx\n");

    // Cursor on the use `x` on line 2.
    let offset = TextSize::from("assign(\"x\", 1)\n".len() as u32);
    let targets = goto_definition(&db, file, offset);
    assert_eq!(targets.len(), 1);
    let target = &targets[0];

    assert_eq!(target.file, file);
    assert_eq!(target.name, "x");
    // Lands on the quoted name argument `"x"`.
    assert_eq!(target.full_range, range(7, 10));
}

#[test]
fn test_navigates_to_delayed_assign_binding() {
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "a.R", "delayedAssign(\"x\", expr)\nx\n");

    let offset = TextSize::from("delayedAssign(\"x\", expr)\n".len() as u32);
    let targets = goto_definition(&db, file, offset);
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].name, "x");
    assert_eq!(targets[0].full_range, range(14, 17));
}
