//! Find-references at the ide layer.
//!
//! Each test wires up a small database, calls `find_references`, and checks
//! the returned `FileRange`s. Resolution coverage (reaching-defs, scope
//! chains, cross-file source) lives in `oak_db`'s tests; we only check the
//! orchestration here: scope decision, confirm step, member scan.
//!
//! Results are deterministically ordered (current file first, then by URL,
//! then by source offset), so tests assert the full result vector rather than
//! membership.

use biome_rowan::TextSize;
use oak_db::OakDatabase;
use oak_ide::find_references;

use crate::support::install_library_package;
use crate::support::install_workspace_package;
use crate::support::offset;
use crate::support::pairs;
use crate::support::range;
use crate::support::ranges;
use crate::support::upsert;

// --- Local resolution ---

#[test]
fn test_local_simple() {
    // "x <- 1\nx\n"
    //  0      7
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "test.R", "x <- 1\nx\n");

    let refs = find_references(&db, file, offset(7), true);
    assert_eq!(ranges(&refs), vec![range(0, 1), range(7, 8)]);
}

#[test]
fn test_local_excludes_declaration() {
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "test.R", "x <- 1\nx\n");

    let refs = find_references(&db, file, offset(7), false);
    assert_eq!(ranges(&refs), vec![range(7, 8)]);
}

#[test]
fn test_from_definition_site() {
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "test.R", "x <- 1\nx\n");

    let refs = find_references(&db, file, offset(0), true);
    assert_eq!(ranges(&refs), vec![range(0, 1), range(7, 8)]);
}

#[test]
fn test_shadowing_excludes_outer() {
    let source = "x <- 1\nf <- function() {\n  x <- 2\n  x\n}\n";
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "test.R", source);

    let inner_def = source.find("x <- 2").unwrap() as u32;
    let inner_use = source.rfind('x').unwrap() as u32;

    // Cursor on the inner use: inner pair only, outer `x` excluded.
    let refs = find_references(&db, file, offset(inner_use), true);
    assert_eq!(ranges(&refs), vec![
        range(inner_def, inner_def + 1),
        range(inner_use, inner_use + 1),
    ]);

    // Cursor on outer def: no uses, just the def.
    let refs = find_references(&db, file, offset(0), true);
    assert_eq!(ranges(&refs), vec![range(0, 1)]);
}

#[test]
fn test_free_variable_in_inner_scope() {
    let source = "x <- 1\nf <- function() {\n  x\n}\n";
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "test.R", source);

    let inner_use = source.rfind('x').unwrap() as u32;
    let refs = find_references(&db, file, offset(inner_use), true);
    assert_eq!(ranges(&refs), vec![
        range(0, 1),
        range(inner_use, inner_use + 1),
    ]);
}

#[test]
fn test_multiple_uses() {
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "test.R", "x <- 1\nx + x + x\n");

    let refs = find_references(&db, file, offset(0), true);
    assert_eq!(ranges(&refs), vec![
        range(0, 1),
        range(7, 8),
        range(11, 12),
        range(15, 16),
    ]);
}

#[test]
fn test_parameter_refs() {
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "test.R", "f <- function(x) {\n  x + x\n}\n");

    let refs = find_references(&db, file, offset(14), true);
    assert_eq!(ranges(&refs), vec![
        range(14, 15),
        range(21, 22),
        range(25, 26)
    ]);
}

#[test]
fn test_reassignment_separates_refs() {
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "test.R", "x <- 1\nx <- 2\nx\n");

    // Second def: its use follows it; first def is killed.
    let refs = find_references(&db, file, offset(7), true);
    assert_eq!(ranges(&refs), vec![range(7, 8), range(14, 15)]);

    // First def: shadowed immediately, no uses.
    let refs = find_references(&db, file, offset(0), true);
    assert_eq!(ranges(&refs), vec![range(0, 1)]);
}

#[test]
fn test_conditional_binding_includes_both_defs() {
    let source = "if (TRUE) x <- 1 else x <- 2\nx\n";
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "test.R", source);

    let use_offset = source.rfind('x').unwrap() as u32;
    let refs = find_references(&db, file, offset(use_offset), true);
    assert_eq!(ranges(&refs), vec![
        range(10, 11),
        range(22, 23),
        range(use_offset, use_offset + 1),
    ]);
}

#[test]
fn test_super_assignment_targets_outer_scope() {
    let source = "x <- 1\nf <- function() x <<- 2\nx\n";
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "test.R", source);

    let super_def = source.find("x <<-").unwrap() as u32;
    let outer_use = source.rfind('x').unwrap() as u32;
    let refs = find_references(&db, file, offset(outer_use), true);
    assert_eq!(ranges(&refs), vec![
        range(0, 1),
        range(super_def, super_def + 1),
        range(outer_use, outer_use + 1),
    ]);
}

// --- Boundary cursor ---

#[test]
fn test_cursor_at_trailing_edge_resolves() {
    // `x` spans 7..8; cursor at 8 (trailing edge).
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "test.R", "x <- 1\nx\n");

    let refs = find_references(&db, file, offset(8), true);
    assert_eq!(ranges(&refs), vec![range(0, 1), range(7, 8)]);
}

// --- No resolution ---

#[test]
fn test_no_identifier_at_offset() {
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "test.R", "x <- 1\n");

    // Cursor on `<-` operator
    let refs = find_references(&db, file, offset(3), true);
    assert!(refs.is_empty());
}

#[test]
fn test_unbound_use_returns_empty() {
    // `foo` has no definition anywhere in the db.
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "test.R", "foo\n");

    let refs = find_references(&db, file, offset(0), true);
    assert!(refs.is_empty());
}

#[test]
fn test_namespace_rhs_returns_namespace_scan() {
    // Cursor on `mutate` RHS of `::` with no `dplyr` package in the db falls
    // back to the structural namespace scan: it matches `dplyr::mutate` across
    // files but not `tidyr::mutate` (different namespace) nor a bare `mutate()`
    // call (no package to resolve through, so there's no shared definition to
    // compare against). The resolve-and-bridge path is exercised by
    // `test_namespace_access_bridges_to_bare_name`.
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "a.R", "dplyr::mutate\n");
    let file2 = upsert(&mut db, "b.R", "dplyr::mutate\ntidyr::mutate\nmutate()\n");

    let refs = find_references(&db, file, offset(7), true);
    // a.R (primary) first, then b.R. b.R's `tidyr::mutate` and bare `mutate`
    // are excluded.
    assert_eq!(pairs(&refs), vec![
        (file, range(7, 13)),
        (file2, range(7, 13)),
    ]);
}

// --- Dollar/at member access ---

#[test]
fn test_dollar_lhs_resolves_only_to_variable() {
    // `foo` on the LHS of `$` is a real variable use. RHS `foo` (18..21)
    // is a member name, not part of the variable's references.
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "test.R", "foo <- list()\nfoo$foo\n");

    let refs = find_references(&db, file, offset(14), true);
    assert_eq!(ranges(&refs), vec![range(0, 3), range(14, 17)]);
}

#[test]
fn test_dollar_rhs_returns_member_scan() {
    // Cursor on `bar` RHS of `$` uses the structural member scan: it matches
    // `$bar` across files but not plain `bar`.
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "a.R", "foo <- list()\nfoo$bar\n");
    let file2 = upsert(&mut db, "b.R", "foo$bar\nbar\n");

    let refs = find_references(&db, file, offset(18), true);
    // a.R (primary) first, then b.R. b.R's plain `bar` at 8..11 is excluded.
    assert_eq!(pairs(&refs), vec![
        (file, range(18, 21)),
        (file2, range(4, 7)),
    ]);
}

#[test]
fn test_string_def_returns_quoted_range_for_def() {
    let mut db = OakDatabase::new();
    let file = upsert(&mut db, "test.R", "\"foo\" <- 1\nfoo\n");

    let refs = find_references(&db, file, offset(11), true);
    assert_eq!(ranges(&refs), vec![range(0, 5), range(11, 14)]);
}

// --- Cross-file variable references ---

#[test]
fn test_cross_file_via_source() {
    // helpers.R defines `helper`; script.R sources it and uses it.
    let mut db = OakDatabase::new();
    let helpers = upsert(&mut db, "helpers.R", "helper <- function() 1\n");
    let script = upsert(&mut db, "script.R", "source(\"helpers.R\")\nhelper\n");

    let use_start = "source(\"helpers.R\")\n".len() as u32;
    let refs = find_references(&db, script, offset(use_start), true);

    // script.R (primary) first with its use, then helpers.R with the def.
    assert_eq!(pairs(&refs), vec![
        (script, range(use_start, use_start + 6)),
        (helpers, range(0, 6)),
    ]);
}

#[test]
fn test_different_binding_not_included() {
    // file2 has its own `foo` binding. Cursor on file1's `foo` must not
    // include file2's `foo` (confirmed distinct by resolve_at).
    let mut db = OakDatabase::new();
    let file1 = upsert(&mut db, "a.R", "foo <- 1\nfoo\n");
    let _file2 = upsert(&mut db, "b.R", "foo <- 99\nfoo\n");

    let refs = find_references(&db, file1, offset(0), true);
    assert_eq!(pairs(&refs), vec![
        (file1, range(0, 3)),
        (file1, range(9, 12)),
    ]);
}

#[test]
fn test_locally_scoped_stays_in_file() {
    // Parameter `x` is function-scoped, so only file1 is searched even though
    // file2 has same-name occurrences.
    let mut db = OakDatabase::new();
    let file1 = upsert(&mut db, "a.R", "f <- function(x) {\n  x + 1\n}\n");
    let _file2 = upsert(&mut db, "b.R", "x <- 99\nx\n");

    let param_offset = TextSize::from("f <- function(".len() as u32);
    let refs = find_references(&db, file1, param_offset, true);
    assert_eq!(pairs(&refs), vec![
        (file1, range(14, 15)),
        (file1, range(21, 22)),
    ]);
}

// --- Bare name <-> namespace bridge ---

#[test]
fn test_package_symbol_bridges_to_namespace_access() {
    // `foo` is defined in workspace package `pkg`. A reference search from the
    // bare name also surfaces `pkg::foo` qualified sites, since they name the
    // same binding. `script.R`'s bare `foo` doesn't resolve to `pkg` (no
    // attach), so it isn't included.
    let mut db = OakDatabase::new();
    let foo_file =
        install_workspace_package(&mut db, "pkg", &["foo"], "foo.R", "foo <- function() 1\n");
    let script = upsert(&mut db, "script.R", "pkg::foo()\nfoo\n");

    // Cursor on the def `foo` at offset 0.
    let refs = find_references(&db, foo_file, offset(0), true);

    // `pkg`'s def (primary) first, then `pkg::foo` in script.R (5..8). The
    // bare `foo` in script.R (11..14) is excluded.
    assert_eq!(pairs(&refs), vec![
        (foo_file, range(0, 3)),
        (script, range(5, 8)),
    ]);
}

#[test]
fn test_namespace_access_bridges_to_bare_name() {
    // The inverse of `test_package_symbol_bridges_to_namespace_access`: a cursor
    // on `mypkg::foo` resolves `foo` in `mypkg` and runs the variable path, so
    // it surfaces the bare `foo` use that attaches `mypkg` too. `script.R` does
    // `library(mypkg)`, so its bare `foo` resolves to the same binding. `mypkg`
    // is a workspace package, so its editable def is included.
    let mut db = OakDatabase::new();
    let pkg_file =
        install_workspace_package(&mut db, "mypkg", &["foo"], "a.R", "foo <- function() 42\n");
    let script = upsert(&mut db, "script.R", "library(mypkg)\nmypkg::foo()\nfoo\n");

    // Cursor on the `foo` in `mypkg::foo` (offset 22).
    let refs = find_references(&db, script, offset(22), true);

    // script.R (primary) first: the `mypkg::foo` qualified site (22..25) then the
    // bare `foo` use (28..31). Then `mypkg`'s def in a.R (0..3).
    assert_eq!(pairs(&refs), vec![
        (script, range(22, 25)),
        (script, range(28, 31)),
        (pkg_file, range(0, 3)),
    ]);
}

#[test]
fn test_namespace_access_excludes_installed_package_def() {
    // Same shape as the workspace bridge, but `mypkg` is an *installed* package.
    // Its source is read-only, so the def in a.R is dropped. Only the workspace
    // sites in script.R survive: the `mypkg::foo` qualified site and the bare
    // `foo` use that resolves through the attach.
    let mut db = OakDatabase::new();
    let _pkg_file =
        install_library_package(&mut db, "mypkg", &["foo"], "a.R", "foo <- function() 42\n");
    let script = upsert(&mut db, "script.R", "library(mypkg)\nmypkg::foo()\nfoo\n");

    let refs = find_references(&db, script, offset(22), true);
    assert_eq!(pairs(&refs), vec![
        (script, range(22, 25)),
        (script, range(28, 31)),
    ]);
}

#[test]
fn test_cursor_in_installed_package_finds_references() {
    // The cursor is in installed-package source itself: the user has navigated
    // into `mypkg` and runs find-references on its `foo`. Here library hits are
    // wanted, so the exclusion is lifted. We get the def and the sibling use
    // inside `mypkg`, plus the bare `foo` in the workspace script that attaches
    // it.
    let mut db = OakDatabase::new();
    let pkg_file = install_library_package(
        &mut db,
        "mypkg",
        &["foo"],
        "a.R",
        "foo <- function() 1\nfoo()\n",
    );
    let script = upsert(&mut db, "script.R", "library(mypkg)\nfoo\n");

    // Cursor on the def `foo` at offset 0 inside the package file.
    let refs = find_references(&db, pkg_file, offset(0), true);

    // a.R (primary, the library file) first: def (0..3) then the sibling use
    // `foo()` (20..23). Then the workspace use in script.R (15..18).
    assert_eq!(pairs(&refs), vec![
        (pkg_file, range(0, 3)),
        (pkg_file, range(20, 23)),
        (script, range(15, 18)),
    ]);
}

#[test]
fn test_cursor_in_installed_package_excludes_other_packages() {
    // Cursor in `mypkg` source again, but now a *second* installed package,
    // `otherpkg`, also calls `mypkg::foo`. References within `mypkg` and in the
    // workspace are kept, but the hit inside `otherpkg` is dropped: it's
    // read-only source the user didn't navigate into.
    let mut db = OakDatabase::new();
    let mypkg_file =
        install_library_package(&mut db, "mypkg", &["foo"], "a.R", "foo <- function() 1\n");
    let _otherpkg_file =
        install_library_package(&mut db, "otherpkg", &[], "b.R", "mypkg::foo()\n");
    let script = upsert(&mut db, "script.R", "mypkg::foo()\n");

    // Cursor on the def `foo` at offset 0 inside `mypkg`.
    let refs = find_references(&db, mypkg_file, offset(0), true);

    // `mypkg`'s def (primary) and the workspace `mypkg::foo` site. `otherpkg`'s
    // `mypkg::foo` is excluded.
    assert_eq!(pairs(&refs), vec![
        (mypkg_file, range(0, 3)),
        (script, range(7, 10)),
    ]);
}

#[test]
fn test_cross_package_references_via_library() {
    // A script attaches `mypkg` and uses its exported `foo`. The use resolves
    // through the package layer to the binding in the package file, confirming
    // the cross-package resolution feeds `resolve_at`. The def lives in an
    // installed package, so it is excluded from the results: only the script
    // use is reported, with or without `include_declaration`.
    let mut db = OakDatabase::new();
    let _pkg_file =
        install_library_package(&mut db, "mypkg", &["foo"], "a.R", "foo <- function() 42\n");
    let script = upsert(&mut db, "script.R", "library(mypkg)\nfoo\n");

    let use_start = "library(mypkg)\n".len() as u32;
    let refs = find_references(&db, script, offset(use_start), true);
    assert_eq!(pairs(&refs), vec![(
        script,
        range(use_start, use_start + 3)
    )]);

    let refs = find_references(&db, script, offset(use_start), false);
    assert_eq!(pairs(&refs), vec![(
        script,
        range(use_start, use_start + 3)
    )]);
}
