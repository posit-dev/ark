//
// dap_breakpoints_line_adjustment.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use ark_test::DummyArkFrontend;
use ark_test::SourceFile;

#[test]
fn test_dap_breakpoint_line_adjustment_multiline_expr() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Breakpoints inside multiline expressions should be adjusted up to expression start
    let file = SourceFile::new(
        "
foo <- function() {
  list(
    1,
    2
  )
}
",
    );

    // Set breakpoints on lines 4 and 5 (inside the list() call)
    // Line 3 is `list(`, lines 4-5 are inside, line 6 is `)`
    let breakpoints = dap.set_breakpoints(&file.path, &[4, 5]);
    assert_eq!(breakpoints.len(), 2);
    let id1 = breakpoints[0].id;
    let id2 = breakpoints[1].id;

    // Source the file to verify breakpoints
    frontend.source_file(&file);

    // Both breakpoints should be verified and adjusted to line 3 (start of `list(`)
    let bp1 = dap.recv_breakpoint_verified();
    let bp2 = dap.recv_breakpoint_verified();

    // Check that both are our breakpoints (order may vary)
    let ids: Vec<_> = vec![bp1.id, bp2.id];
    assert!(ids.contains(&id1));
    assert!(ids.contains(&id2));

    // Both should be adjusted to line 3 (the start of the multiline expression)
    assert_eq!(bp1.line, Some(3));
    assert_eq!(bp2.line, Some(3));
}

#[test]
fn test_dap_breakpoint_line_adjustment_blank_line() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Breakpoints on whitespace/comments should be adjusted down to next statement
    let file = SourceFile::new(
        "
foo <- function() {
  # comment

  1
}
",
    );

    // Set breakpoints on line 3 (comment) and line 4 (blank line)
    // They should both be adjusted down to line 5 (the `1` statement)
    let breakpoints = dap.set_breakpoints(&file.path, &[3, 4]);
    assert_eq!(breakpoints.len(), 2);
    let id1 = breakpoints[0].id;
    let id2 = breakpoints[1].id;

    // Source the file to verify breakpoints
    frontend.source_file(&file);

    // Both breakpoints should be verified and adjusted to line 5 (the next statement)
    let bp1 = dap.recv_breakpoint_verified();
    let bp2 = dap.recv_breakpoint_verified();

    // Check that both are our breakpoints (order may vary)
    let ids: Vec<_> = vec![bp1.id, bp2.id];
    assert!(ids.contains(&id1));
    assert!(ids.contains(&id2));

    // Both should be adjusted to line 5 (the `1` statement)
    assert_eq!(bp1.line, Some(5));
    assert_eq!(bp2.line, Some(5));
}

#[test]
fn test_dap_breakpoints_anchor_to_same_line() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Multiple breakpoints that anchor to the same expression should all be verified together
    let file = SourceFile::new(
        "
foo <- function() {
  # comment
  list(
    1
  )
}
",
    );

    // Set breakpoints on lines 3 (comment), 4 (list(), and 5 (inside list)
    // All should anchor to line 4 (the `list(` statement)
    let breakpoints = dap.set_breakpoints(&file.path, &[3, 4, 5]);
    assert_eq!(breakpoints.len(), 3);
    let id1 = breakpoints[0].id;
    let id2 = breakpoints[1].id;
    let id3 = breakpoints[2].id;

    // Source the file to verify breakpoints
    frontend.source_file(&file);

    // All three breakpoints should be verified
    let bp1 = dap.recv_breakpoint_verified();
    let bp2 = dap.recv_breakpoint_verified();
    let bp3 = dap.recv_breakpoint_verified();

    // Check that all are our breakpoints (order may vary)
    let ids: Vec<_> = vec![bp1.id, bp2.id, bp3.id];
    assert!(ids.contains(&id1));
    assert!(ids.contains(&id2));
    assert!(ids.contains(&id3));

    // All should be adjusted to line 4 (the `list(` expression start)
    assert_eq!(bp1.line, Some(4));
    assert_eq!(bp2.line, Some(4));
    assert_eq!(bp3.line, Some(4));
}
