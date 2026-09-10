// Copyright (C) 2026 by Posit Software, PBC

use std::process::Command;

/// Run with `just test -p ark --test integration reticulate -- --ignored`.
/// POSITRON_PYTHON_FILES must contain Positron's built Python 3.14 bundle;
/// RETICULATE_TEST_PYTHON must point to Python 3.14. R needs reticulate and processx.
#[test]
#[ignore = "requires reticulate, Python 3.14, and Positron's built kernel bundle"]
fn test_reticulate_kernels() -> anyhow::Result<()> {
    let files = std::env::var("POSITRON_PYTHON_FILES")?;
    let python = std::env::var("RETICULATE_TEST_PYTHON")?;
    for mode in ["project", "managed", "unbundled"] {
        let output = Command::new("Rscript")
            .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .args([
                "--vanilla",
                "crates/ark/tests/integration/reticulate-kernel.R",
                &files,
                &python,
                mode,
            ])
            .output()?;
        eprintln!(
            "{mode}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.status.success());
    }
    Ok(())
}
