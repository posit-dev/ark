use std::ffi::OsString;
use std::fs::File;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Child;
use std::process::Command;
use std::process::ExitStatus;
use std::process::Output;
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

#[cfg(unix)]
use nix::sys::signal::killpg;
#[cfg(unix)]
use nix::sys::signal::Signal;
#[cfg(unix)]
use nix::unistd::Pid;

static STARTUP_ENVIRONMENT: OnceLock<Vec<(OsString, OsString)>> = OnceLock::new();

const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Which startup files the R subprocess runs.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// Skips environment files and site and user profiles via `--vanilla`.
    #[default]
    Vanilla,

    /// Uses R's normal environment-file and profile discovery, including
    /// environment-variable overrides and working-directory or home-directory
    /// lookup. Does not restore or save a workspace.
    Startup,
}

#[derive(Debug, Default)]
pub struct RunOptions<'a> {
    pub profile: Profile,

    /// Inherits this process's working directory when `None`.
    pub working_dir: Option<&'a Path>,

    /// Limits the wait after spawning. On expiry, attempts to kill the process
    /// tree, waits for the direct child to exit, and returns an error.
    /// Waits indefinitely when `None`.
    pub timeout: Option<Duration>,

    /// Available through `commandArgs()` with `trailingOnly = TRUE`.
    pub args: &'a [&'a str],

    /// Overrides the captured environment, or the current environment if no
    /// snapshot has been captured. R startup files may change these values.
    pub env: &'a [(&'a str, &'a str)],
}

/// Captures the base environment for subsequent calls to [`run_file()`] and
/// [`run_text()`]. The first capture wins.
///
/// Call after setting `R_HOME` but before starting the embedded R session.
/// Session changes such as renv setting `R_LIBS_USER` would otherwise leak the
/// session's library paths into subprocesses. Without a snapshot, subprocesses
/// inherit the current environment. [`RunOptions::env`] overrides either base.
pub fn capture_startup_environment() {
    if STARTUP_ENVIRONMENT
        .set(std::env::vars_os().collect())
        .is_err()
    {
        log::warn!("Startup environment for R subprocesses was already captured");
    }
}

/// Runs an R file in a subprocess via `R` and returns its output.
///
/// Stdin is null, so reads reach EOF rather than waiting for frontend input.
/// This does not guarantee that R input functions return an error.
pub fn run_file(r: &Path, file: &Path, options: &RunOptions) -> anyhow::Result<Output> {
    let mut command = Command::new(r);

    command.arg("-f").arg(file);

    match options.profile {
        Profile::Vanilla => {
            // `--no-save`, `--no-restore`, `--no-site-file`, `--no-init-file`, `--no-environ`
            command.arg("--vanilla");
        },
        Profile::Startup => {
            command.arg("--no-save").arg("--no-restore");
        },
    }

    command.arg("--no-echo");

    if !options.args.is_empty() {
        command.arg("--args").args(options.args);
    }

    if let Some(dir) = options.working_dir {
        command.current_dir(dir);
    }

    if let Some(environment) = STARTUP_ENVIRONMENT.get() {
        command.env_clear().envs(environment.iter().cloned());
    }
    for (key, value) in options.env {
        command.env(key, value);
    }

    // Temporary files prevent output writes from blocking on a full pipe buffer.
    // Neither `wait_with_timeout()` nor `Child::wait()` drains output while waiting.
    let mut stdout = tempfile::tempfile()
        .map_err(|err| anyhow::anyhow!("Failed to create temporary file: {err}"))?;
    let mut stderr = tempfile::tempfile()
        .map_err(|err| anyhow::anyhow!("Failed to create temporary file: {err}"))?;

    command
        .stdin(Stdio::null())
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?);

    set_own_process_group(&mut command);

    let mut child = command.spawn()?;

    let status = match options.timeout {
        Some(timeout) => wait_with_timeout(&mut child, timeout)?,
        None => child.wait()?,
    };

    Ok(Output {
        status,
        stdout: read_from_start(&mut stdout)?,
        stderr: read_from_start(&mut stderr)?,
    })
}

/// Runs a snippet of R code in a subprocess via `R` and returns its output.
///
/// Writes the snippet to a temp file, because using `-e` is very unreliable regarding
/// how things are quoted, particularly on Windows
pub fn run_text(r: &Path, text: &str, options: &RunOptions) -> anyhow::Result<Output> {
    let file = write_tempfile(text)?;
    run_file(r, file.path(), options)
}

fn wait_with_timeout(child: &mut Child, timeout: Duration) -> anyhow::Result<ExitStatus> {
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    if let Err(err) = kill_tree(child) {
        log::error!("Can't kill R subprocess tree, killing the direct child only: {err:?}");
        child.kill()?;
    }

    child.wait()?;

    Err(anyhow::anyhow!(
        "R subprocess timed out after {}s",
        timeout.as_secs_f64()
    ))
}

/// Gives the child a process group identified by its PID so `kill_tree()` can
/// use `killpg()` to reach descendants even after their parents exit. Descendants
/// that leave the group are not reached.
#[cfg(unix)]
fn set_own_process_group(command: &mut Command) {
    command.process_group(0);
}

#[cfg(windows)]
fn set_own_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn kill_tree(child: &Child) -> anyhow::Result<()> {
    let pgid = Pid::from_raw(i32::try_from(child.id())?);
    killpg(pgid, Signal::SIGKILL)?;

    Ok(())
}

/// `taskkill /T` walks the tree by parent PID. It covers `R.exe`, a launcher that
/// waits on its `Rterm.exe` child, but misses a descendant whose parent has already
/// exited.
#[cfg(windows)]
fn kill_tree(child: &Child) -> anyhow::Result<()> {
    let status = Command::new("taskkill")
        .args(["/T", "/F", "/PID"])
        .arg(child.id().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;

    if !status.success() {
        return Err(anyhow::anyhow!("`taskkill` failed with {status}"));
    }

    Ok(())
}

/// Rewinds the offset shared with the child's duplicated handle so reads include
/// the output already written.
fn read_from_start(file: &mut File) -> anyhow::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(0))?;

    let mut contents = Vec::new();
    file.read_to_end(&mut contents)?;

    Ok(contents)
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

#[cfg(test)]
mod tests;
