//
// dap_breakpoints_symlink.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use std::io::Write;

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use amalthea::wire::execute_request::ExecuteRequestPositron;
use amalthea::wire::execute_request::JupyterPositronLocation;
use amalthea::wire::execute_request::JupyterPositronPosition;
use amalthea::wire::execute_request::JupyterPositronRange;
use ark_test::DummyArkFrontend;
#[cfg(target_os = "macos")]
use ark_test::SourceFile;
use url::Url;

/// A temp file accessible through both a real path and a symlinked path.
///
/// Creates a real directory with a file, and a symlink directory pointing
/// to it. The `real_path` goes through the actual directory, while
/// `symlink_path` goes through the symlink. Both refer to the same file.
struct SymlinkedFile {
    real_path: String,
    symlink_path: String,
    _real_dir: tempfile::TempDir,
    _symlink_dir: tempfile::TempDir,
}

impl SymlinkedFile {
    fn new(code: &str) -> Self {
        // Create the real directory and write the file
        let real_dir = tempfile::tempdir().unwrap();
        let real_file = real_dir.path().join("test.R");
        let mut f = std::fs::File::create(&real_file).unwrap();
        write!(f, "{code}").unwrap();
        drop(f);

        // Create a separate temp dir, remove it, and place a symlink there
        let symlink_dir = tempfile::tempdir().unwrap();
        let symlink_target = symlink_dir.path().join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(real_dir.path(), &symlink_target).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(real_dir.path(), &symlink_target).unwrap();

        let symlink_file = symlink_target.join("test.R");

        // Sanity: both paths read the same content
        assert_eq!(
            std::fs::read_to_string(&real_file).unwrap(),
            std::fs::read_to_string(&symlink_file).unwrap(),
        );

        // Sanity: the paths are actually different strings
        let real_path = real_file.to_string_lossy().replace('\\', "/");
        let symlink_path = symlink_file.to_string_lossy().replace('\\', "/");
        assert_ne!(real_path, symlink_path);

        Self {
            real_path,
            symlink_path,
            _real_dir: real_dir,
            _symlink_dir: symlink_dir,
        }
    }

    fn line_count(&self) -> u32 {
        std::fs::read_to_string(&self.real_path)
            .unwrap()
            .lines()
            .count() as u32
    }

    fn code(&self) -> String {
        std::fs::read_to_string(&self.real_path).unwrap()
    }
}

fn make_location(path: &str, line_count: u32) -> JupyterPositronLocation {
    let uri = Url::from_file_path(path).unwrap();
    JupyterPositronLocation {
        uri: uri.to_string(),
        range: JupyterPositronRange {
            start: JupyterPositronPosition {
                line: 0,
                character: 0,
            },
            end: JupyterPositronPosition {
                line: line_count,
                character: 0,
            },
        },
    }
}

fn symlink_location(file: &SymlinkedFile) -> JupyterPositronLocation {
    make_location(&file.symlink_path, file.line_count())
}

fn real_location(file: &SymlinkedFile) -> JupyterPositronLocation {
    make_location(&file.real_path, file.line_count())
}

/// Test that breakpoints activate on execute requests when the frontend
/// sends a URI through a symlink that differs from the canonical path.
///
/// DAP's `SetBreakpoints` canonicalizes paths via `UrlId::from_file_path`.
/// The execute request's URI comes from the frontend as a raw string.
/// `UrlId` must resolve both to the same canonical key.
///
/// This test creates an explicit symlink to avoid relying on OS-specific
/// symlinks (e.g. macOS `/var` -> `/private/var`).
#[test]
fn test_dap_breakpoint_hit_via_symlink_execute() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let file = SymlinkedFile::new(
        "
foo <- function() {
  x <- 1
  x + 1
}
foo()
",
    );

    // Set breakpoint via the symlinked path. DAP canonicalizes this to
    // the real path internally.
    let breakpoints = dap.set_breakpoints(&file.symlink_path, &[3]);
    assert_eq!(breakpoints.len(), 1);
    assert!(!breakpoints[0].verified);
    let bp_id = breakpoints[0].id;

    // Send execute request with the symlinked URI (as Positron would).
    // `UrlId::from_code_location` must resolve this to the same canonical
    // key that `set_breakpoints` used.
    frontend.send_execute_request(&file.code(), ExecuteRequestOptions {
        positron: Some(ExecuteRequestPositron {
            code_location: Some(symlink_location(&file)),
        }),
        ..Default::default()
    });
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp_id);
    assert_eq!(bp.line, Some(3));

    frontend.recv_iopub_breakpoint_hit();

    dap.recv_stopped();
    dap.assert_top_frame("foo()");
    dap.assert_top_frame_line(3);

    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}

/// Test that breakpoints set via the symlinked path are found when the
/// execute request uses the real path.
#[test]
fn test_dap_breakpoint_symlink_set_real_execute() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let file = SymlinkedFile::new(
        "
norf <- function() {
  w <- 7
  w
}
norf()
",
    );

    // Set breakpoint via the symlinked path
    let breakpoints = dap.set_breakpoints(&file.symlink_path, &[3]);
    assert_eq!(breakpoints.len(), 1);
    assert!(!breakpoints[0].verified);
    let bp_id = breakpoints[0].id;

    // Execute via the real path
    frontend.send_execute_request(&file.code(), ExecuteRequestOptions {
        positron: Some(ExecuteRequestPositron {
            code_location: Some(real_location(&file)),
        }),
        ..Default::default()
    });
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp_id);
    assert_eq!(bp.line, Some(3));

    frontend.recv_iopub_breakpoint_hit();

    dap.recv_stopped();
    dap.assert_top_frame("norf()");
    dap.assert_top_frame_line(3);

    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}

/// Test that breakpoints set via the real path are found when the execute
/// request uses the symlinked path.
#[test]
fn test_dap_breakpoint_real_set_symlink_execute() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let file = SymlinkedFile::new(
        "
qux <- function() {
  z <- 99
  z
}
qux()
",
    );

    // Set breakpoint via the *real* path
    let breakpoints = dap.set_breakpoints(&file.real_path, &[3]);
    assert_eq!(breakpoints.len(), 1);
    assert!(!breakpoints[0].verified);
    let bp_id = breakpoints[0].id;

    // Execute via the *symlinked* path
    frontend.send_execute_request(&file.code(), ExecuteRequestOptions {
        positron: Some(ExecuteRequestPositron {
            code_location: Some(symlink_location(&file)),
        }),
        ..Default::default()
    });
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp_id);
    assert_eq!(bp.line, Some(3));

    frontend.recv_iopub_breakpoint_hit();

    dap.recv_stopped();
    dap.assert_top_frame("qux()");
    dap.assert_top_frame_line(3);

    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}

// --- macOS-specific tests using the implicit `/var` → `/private/var` symlink ---
//
// On macOS, `tempfile::tempdir()` creates directories under `/var/folders/...`
// which is a symlink to `/private/var/folders/...`. This means `SourceFile`
// paths are non-canonical by default, exercising the same codepath that
// Positron hits in practice.

/// Build a `JupyterPositronLocation` from a `SourceFile`'s raw (non-canonical)
/// path, as Positron would send it.
#[cfg(target_os = "macos")]
fn source_file_location(file: &SourceFile) -> JupyterPositronLocation {
    let line_count = std::fs::read_to_string(&file.path).unwrap().lines().count() as u32;
    let uri = Url::from_file_path(&file.path).unwrap();

    JupyterPositronLocation {
        uri: uri.to_string(),
        range: JupyterPositronRange {
            start: JupyterPositronPosition {
                line: 0,
                character: 0,
            },
            end: JupyterPositronPosition {
                line: line_count,
                character: 0,
            },
        },
    }
}

/// Test that breakpoints work through macOS's implicit `/var` → `/private/var`
/// symlink, which is what Positron hits in practice when temp files are involved.
#[test]
#[cfg(target_os = "macos")]
fn test_dap_breakpoint_hit_via_macos_var_symlink() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let file = SourceFile::new(
        "
greet <- function() {
  msg <- 'hello'
  msg
}
greet()
",
    );

    // Sanity: on macOS, SourceFile paths go through `/var/folders/...`
    assert!(
        file.path.starts_with("/var/"),
        "Expected /var/ prefix, got: {}",
        file.path
    );

    let breakpoints = dap.set_breakpoints(&file.path, &[3]);
    assert_eq!(breakpoints.len(), 1);
    assert!(!breakpoints[0].verified);
    let bp_id = breakpoints[0].id;

    let code = std::fs::read_to_string(&file.path).unwrap();
    frontend.send_execute_request(&code, ExecuteRequestOptions {
        positron: Some(ExecuteRequestPositron {
            code_location: Some(source_file_location(&file)),
        }),
        ..Default::default()
    });
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp_id);
    assert_eq!(bp.line, Some(3));

    frontend.recv_iopub_breakpoint_hit();

    dap.recv_stopped();
    dap.assert_top_frame("greet()");
    dap.assert_top_frame_line(3);

    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}

/// Test that `source()` works through macOS's `/var` symlink.
///
/// R's `normalizePath()` resolves symlinks on its own, so this path
/// should work independently of `UrlId` canonicalization. This test
/// guards against regressions.
#[test]
#[cfg(target_os = "macos")]
fn test_dap_breakpoint_hit_via_macos_var_symlink_source() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let file = SourceFile::new(
        "
calc <- function() {
  a <- 10
  a * 2
}
calc()
",
    );

    assert!(
        file.path.starts_with("/var/"),
        "Expected /var/ prefix, got: {}",
        file.path
    );

    let breakpoints = dap.set_breakpoints(&file.path, &[3]);
    assert_eq!(breakpoints.len(), 1);
    assert!(!breakpoints[0].verified);
    let bp_id = breakpoints[0].id;

    frontend.source_file_and_hit_breakpoint(&file);

    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp_id);
    assert_eq!(bp.line, Some(3));

    dap.recv_stopped();
    dap.assert_top_frame("calc()");
    dap.assert_top_frame_line(3);
    dap.assert_top_frame_file(&file);

    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}
