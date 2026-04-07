use std::path::Path;
use std::process::Command;
use std::process::Output;
use std::process::Stdio;

/// Runs an R script in a subprocess via `Rscript` and returns its output.
pub fn run_script(
    rscript: &Path,
    script: &Path,
    args: &[&str],
    env: &[(&str, &str)],
) -> anyhow::Result<Output> {
    let mut command = Command::new(rscript);

    command
        .arg("--vanilla")
        .arg(script)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    for (key, value) in env {
        command.env(key, value);
    }

    let child = command.spawn()?;
    let output = child.wait_with_output()?;

    Ok(output)
}
