//! Checks resolver and recovery restrictions through the actual database
//! parameters. The control build verifies that allowed operations compile.

use std::env;
use std::process::Command;
use std::process::Output;

/// Cargo reports diagnostic paths relative to the workspace root.
const PROBE_FILES: &[&str] = &[
    "crates/oak_db/src/imports/resolver_probe.rs",
    "crates/oak_db/src/file.rs",
    "crates/oak_db/src/file_imports.rs",
    "crates/oak_db/src/load_context.rs",
];

const PROHIBITED_FORM_COUNT: usize = 14;

/// Passing the cfg through `cargo rustc` leaves dependencies cached.
/// Setting `RUSTFLAGS` would rebuild them too. `CARGO_TERM_COLOR` disables
/// color without conflicting with Cargo's rustc `--json` argument.
fn compile_with_cfg(value: &str) -> Output {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    Command::new(cargo)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("CARGO_TERM_COLOR", "never")
        .args([
            "rustc",
            "-p",
            "oak_db",
            "--lib",
            "--message-format=short",
            "--",
            "--cfg",
            &format!("resolver_boundary=\"{value}\""),
            "--emit=metadata",
        ])
        .output()
        .unwrap()
}

#[test]
fn test_allowed_forms_compile() {
    let output = compile_with_cfg("control");
    assert!(output.status.success());
}

#[test]
fn test_prohibited_forms_fail_to_compile() {
    let output = compile_with_cfg("probe");
    assert!(!output.status.success());

    // Use the same diagnostic paths on Windows and Unix for filtering and snapshots.
    let stderr = String::from_utf8_lossy(&output.stderr).replace('\\', "/");
    // Only probe locations belong in the snapshot, not Cargo's error summary.
    let mut lines: Vec<&str> = stderr
        .lines()
        .filter(|line| PROBE_FILES.iter().any(|path| line.starts_with(path)))
        .collect();
    lines.sort_unstable();
    assert_eq!(lines.len(), PROHIBITED_FORM_COUNT);

    insta::with_settings!({description => "\
    Each line is one prohibited form rejected in the resolver probes or an actual \
    recovery helper. Line numbers shift when those files are edited. A NEW or MISSING \
    line means the boundary itself changed and needs review, not a blind `cargo insta accept`."}, {
        insta::assert_snapshot!(lines.join("\n"));
    });
}
