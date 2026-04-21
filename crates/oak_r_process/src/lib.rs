use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::process::Output;
use std::process::Stdio;

/// Runs an R file in a subprocess via `R` and returns its output.
pub fn run_file(
    r: &Path,
    file: &Path,
    args: &[&str],
    env: &[(&str, &str)],
) -> anyhow::Result<Output> {
    let mut command = Command::new(r);

    command.arg("-f").arg(file);

    // `--no-save`, `--no-restore`, `--no-site-file`, `--no-init-file`, `--no-environ`
    command.arg("--vanilla");
    // In addition to `--vanilla`, run as quietly as possible
    command.arg("--no-echo");

    // Optional arguments retrievable via `commandArgs(trailingOnly = TRUE)`
    if !args.is_empty() {
        command.arg("--args").args(args);
    }

    // We collect stdout/stderr into `output`
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    for (key, value) in env {
        command.env(key, value);
    }

    let child = command.spawn()?;
    let output = child.wait_with_output()?;

    Ok(output)
}

/// Runs a snippet of R code in a subprocess via `R` and returns its output.
///
/// Writes the snippet to a temp file, because using `-e` is very unreliable regarding
/// how things are quoted, particularly on Windows
pub fn run_text(
    r: &Path,
    text: &str,
    args: &[&str],
    env: &[(&str, &str)],
) -> anyhow::Result<Output> {
    let file = write_tempfile(text)?;
    run_file(r, file.path(), args, env)
}

fn write_tempfile(text: &str) -> anyhow::Result<tempfile::NamedTempFile> {
    let mut file = tempfile::Builder::new()
        .suffix(".R")
        .tempfile()
        .map_err(|err| anyhow::anyhow!("Failed to create temporary file: {err}"))?;

    file.write_all(text.as_bytes())
        .map_err(|err| anyhow::anyhow!("Failed to write temporary file: {err}"))?;

    Ok(file)
}
