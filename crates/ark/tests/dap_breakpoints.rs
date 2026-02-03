//
// dap_breakpoints.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use ark_test::DummyArkFrontend;
use ark_test::SourceFile;

#[test]
fn test_dap_set_breakpoints_unverified() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let file = SourceFile::new(
        "1
2
3
",
    );

    // Set breakpoints before sourcing - they should be unverified
    let breakpoints = dap.set_breakpoints(&file.path, &[2]);
    assert_eq!(breakpoints.len(), 1);
    assert!(!breakpoints[0].verified);
    assert_eq!(breakpoints[0].line, Some(2));
}

#[test]
fn test_dap_clear_breakpoints() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let file = SourceFile::new(
        "x <- 1
y <- 2
z <- 3
",
    );

    // Set a breakpoint
    let breakpoints = dap.set_breakpoints(&file.path, &[2]);
    assert_eq!(breakpoints.len(), 1);

    // Clear all breakpoints by sending empty list
    let breakpoints = dap.set_breakpoints(&file.path, &[]);
    assert!(breakpoints.is_empty());
}

#[test]
fn test_dap_breakpoint_preserves_state_on_resubmit() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let file = SourceFile::new(
        "a <- 1
b <- 2
c <- 3
",
    );

    // Set initial breakpoints
    let breakpoints = dap.set_breakpoints(&file.path, &[2, 3]);
    assert_eq!(breakpoints.len(), 2);
    let id1 = breakpoints[0].id;
    let id2 = breakpoints[1].id;

    // Re-submit the same breakpoints - IDs should be preserved
    let breakpoints = dap.set_breakpoints(&file.path, &[2, 3]);
    assert_eq!(breakpoints.len(), 2);
    assert_eq!(breakpoints[0].id, id1);
    assert_eq!(breakpoints[1].id, id2);
}

#[test]
fn test_dap_breakpoint_disabled_preserved_and_restored() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Create file with a function we can set breakpoints on
    let file = SourceFile::new(
        "
bar <- function() {
  a <- 1
  b <- 2
  c <- 3
}
",
    );

    // Set breakpoint BEFORE sourcing (on line 4: b <- 2)
    let breakpoints = dap.set_breakpoints(&file.path, &[4]);
    assert_eq!(breakpoints.len(), 1);
    let original_id = breakpoints[0].id;

    // Source the file - breakpoint becomes verified during evaluation
    frontend.source_file(&file);

    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, original_id);

    // Now "disable" the breakpoint by omitting it from the request
    let breakpoints = dap.set_breakpoints(&file.path, &[]);
    assert!(breakpoints.is_empty());

    // Re-enable by submitting the same line again.
    // The breakpoint should have the same ID and be immediately verified
    // (restored from disabled state without needing re-sourcing).
    let breakpoints = dap.set_breakpoints(&file.path, &[4]);
    assert_eq!(breakpoints.len(), 1);
    assert_eq!(breakpoints[0].id, original_id);
    assert!(breakpoints[0].verified);
}

/// Test that document hash changes cause breakpoint state to be discarded.
///
/// This is part of the document change invalidation coverage: when a file's
/// content changes (detected via hash), breakpoint IDs are regenerated and
/// state is reset. This complements the LSP `did_change_document()` unit tests
/// in `dap.rs`.
#[test]
fn test_dap_breakpoint_doc_hash_change_discards_state() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Create initial file
    let mut file = SourceFile::new(
        "x <- 1
y <- 2
z <- 3
",
    );

    // Set breakpoints and record IDs
    let breakpoints = dap.set_breakpoints(&file.path, &[2, 3]);
    assert_eq!(breakpoints.len(), 2);
    let id1 = breakpoints[0].id;
    let id2 = breakpoints[1].id;

    // Modify the file content (different hash)
    file.rewrite("a <- 10\nb <- 20\nc <- 30\n");

    // Re-submit breakpoints at the same lines
    let breakpoints = dap.set_breakpoints(&file.path, &[2, 3]);
    assert_eq!(breakpoints.len(), 2);

    // IDs should be new (state was discarded due to hash change)
    assert_ne!(breakpoints[0].id, id1);
    assert_ne!(breakpoints[1].id, id2);

    // Breakpoints should be unverified since they're new
    assert!(!breakpoints[0].verified);
    assert!(!breakpoints[1].verified);
}

#[test]
fn test_dap_breakpoints_isolated_per_file() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Create two separate files
    let file_a = SourceFile::new(
        "
foo <- function() {
  1
}
",
    );

    let file_b = SourceFile::new(
        "
bar <- function() {
  2
}
",
    );

    // Set breakpoints in both files
    let breakpoints_a = dap.set_breakpoints(&file_a.path, &[3]);
    assert_eq!(breakpoints_a.len(), 1);
    let id_a = breakpoints_a[0].id;

    let breakpoints_b = dap.set_breakpoints(&file_b.path, &[3]);
    assert_eq!(breakpoints_b.len(), 1);
    let id_b = breakpoints_b[0].id;

    // Source only file A
    frontend.source_file(&file_a);

    // Only file A's breakpoint should be verified
    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, id_a);

    // Re-query file B's breakpoints - should still be unverified
    let breakpoints_b = dap.set_breakpoints(&file_b.path, &[3]);
    assert_eq!(breakpoints_b.len(), 1);
    assert_eq!(breakpoints_b[0].id, id_b);
    assert!(!breakpoints_b[0].verified);

    // Clear file A's breakpoints
    let breakpoints_a = dap.set_breakpoints(&file_a.path, &[]);
    assert!(breakpoints_a.is_empty());

    // File B's breakpoints should still exist
    let breakpoints_b = dap.set_breakpoints(&file_b.path, &[3]);
    assert_eq!(breakpoints_b.len(), 1);
    assert_eq!(breakpoints_b[0].id, id_b);
}
