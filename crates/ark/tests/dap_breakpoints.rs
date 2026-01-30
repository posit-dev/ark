//
// dap_breakpoints.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use std::io::Write;

use ark_test::DummyArkFrontend;
use tempfile::NamedTempFile;

/// Create a temp file with given content and return the file and its path.
fn create_temp_file(code: &str) -> (NamedTempFile, String) {
    let mut file = NamedTempFile::new().unwrap();
    write!(file, "{code}").unwrap();
    let path = file.path().to_str().unwrap().replace("\\", "/");
    (file, path)
}

#[test]
fn test_dap_set_breakpoints_unverified() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let (_file, path) = create_temp_file(
        "1
2
3
",
    );

    // Set breakpoints before sourcing - they should be unverified
    let breakpoints = dap.set_breakpoints(&path, &[2]);
    assert_eq!(breakpoints.len(), 1);
    assert!(
        !breakpoints[0].verified,
        "Breakpoint should be unverified before sourcing"
    );
    assert_eq!(breakpoints[0].line, Some(2));
}

#[test]
fn test_dap_clear_breakpoints() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let (_file, path) = create_temp_file(
        "x <- 1
y <- 2
z <- 3
",
    );

    // Set a breakpoint
    let breakpoints = dap.set_breakpoints(&path, &[2]);
    assert_eq!(breakpoints.len(), 1);

    // Clear all breakpoints by sending empty list
    let breakpoints = dap.set_breakpoints(&path, &[]);
    assert!(breakpoints.is_empty());
}

#[test]
fn test_dap_breakpoint_preserves_state_on_resubmit() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let (_file, path) = create_temp_file(
        "a <- 1
b <- 2
c <- 3
",
    );

    // Set initial breakpoints
    let breakpoints = dap.set_breakpoints(&path, &[2, 3]);
    assert_eq!(breakpoints.len(), 2);
    let id1 = breakpoints[0].id;
    let id2 = breakpoints[1].id;

    // Re-submit the same breakpoints - IDs should be preserved
    let breakpoints = dap.set_breakpoints(&path, &[2, 3]);
    assert_eq!(breakpoints.len(), 2);
    assert_eq!(breakpoints[0].id, id1);
    assert_eq!(breakpoints[1].id, id2);
}
