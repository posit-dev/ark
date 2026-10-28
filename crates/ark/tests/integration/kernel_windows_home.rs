/*
 * kernel_windows_home.rs
 *
 * Copyright (C) 2026 Posit Software, PBC. All rights reserved.
 *
 */

//! Startup with non-ASCII `R_HOME` and `R_USER` paths on Windows.
//! https://github.com/posit-dev/positron/issues/12303
//!
//! The kernel runs in the test process, which has ark's UTF-8 manifest embedded
//! (see `build.rs`), so these exercise the same encoding rules as ark.exe.

use std::fs;
use std::path::Path;
use std::process::Command;

use ark_test::DummyArkFrontend;

/// Before posit-dev/ark#1453, the non-ASCII `R_HOME` was mangled
#[test]
fn test_non_ascii_r_home() {
    let r_home = match harp::command::r_home_setup() {
        Ok(r_home) => r_home,
        Err(err) => panic!("Can't find R home: {err:?}"),
    };

    // Re-point the located R installation at a non-ASCII folder via a Windows
    // junction. We remove the junction at the end of the test.
    let dir = tempfile::tempdir().unwrap().keep();
    let junction = dir.join("Strøm");
    create_junction(&junction, &r_home);

    // `sys::windows::home::r_home()` will find this on R startup
    unsafe { std::env::set_var("R_HOME", &junction) };

    let frontend = DummyArkFrontend::lock();

    frontend.execute_request("basename(R.home())", |result| {
        assert_eq!(result, r#"[1] "Strøm""#);
    });
    frontend.execute_request("basename(Sys.getenv('R_HOME'))", |result| {
        assert_eq!(result, r#"[1] "Strøm""#);
    });
    frontend.execute_request("file.exists(R.home('etc'))", |result| {
        assert_eq!(result, "[1] TRUE");
    });

    // Removes the junction
    let _ = fs::remove_dir(&junction);
}

/// Before posit-dev/ark#1453, the non-ASCII `R_USER` was mangled
#[test]
fn test_non_ascii_r_user() {
    // Point `R_USER` at a non-ASCII folder via a Windows junction. We remove
    // the junction at the end of the test.
    let dir = tempfile::tempdir().unwrap();
    let r_user = dir.path().join("Strøm");
    fs::create_dir(&r_user).unwrap();

    // `sys::windows::home::user_home()` will find this on R startup
    unsafe { std::env::set_var("R_USER", &r_user) };

    let frontend = DummyArkFrontend::lock();

    frontend.execute_request("basename(Sys.getenv('R_USER'))", |result| {
        assert_eq!(result, r#"[1] "Strøm""#);
    });
    frontend.execute_request("basename(path.expand('~'))", |result| {
        assert_eq!(result, r#"[1] "Strøm""#);
    });
    frontend.execute_request("dir.exists('~')", |result| {
        assert_eq!(result, "[1] TRUE");
    });
}

/// Directory junctions don't need the symlink privilege, unlike
/// `std::os::windows::fs::symlink_dir()`
fn create_junction(junction: &Path, target: &Path) {
    let status = Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(junction)
        .arg(target)
        .status()
        .unwrap();

    assert!(status.success());
}
