//! These tests require R on the `PATH`.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use std::time::Instant;

use super::*;

fn find_r() -> PathBuf {
    // Use `where` on Windows for native paths. Git's `which` returns POSIX paths
    // that `Command::new()` cannot resolve.
    let output = Command::new(if cfg!(windows) { "where" } else { "which" })
        .arg("R")
        .output()
        .unwrap();
    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    PathBuf::from(stdout.trim().lines().next().unwrap())
}

fn run_stdout(text: &str, options: &RunOptions) -> String {
    let output = run_text(&find_r(), text, options).unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn test_timeout_kills_child() {
    let start = Instant::now();

    let err = run_text(&find_r(), "Sys.sleep(60)", &RunOptions {
        timeout: Some(Duration::from_secs(1)),
        ..Default::default()
    })
    .unwrap_err();

    assert_eq!(err.to_string(), "R subprocess timed out after 1s");
    assert!(start.elapsed() < Duration::from_secs(30));
}

#[test]
fn test_no_timeout_returns_output() {
    let output = run_stdout("cat('hello')", &RunOptions {
        timeout: Some(Duration::from_secs(30)),
        ..Default::default()
    });
    assert_eq!(output, "hello");

    let output = run_stdout("cat('hello')", &RunOptions::default());
    assert_eq!(output, "hello");
}

#[test]
fn test_output_larger_than_pipe_buffer() {
    // An unread pipe would block the child once its buffer filled, preventing
    // completion before the timeout.
    let output = run_stdout("cat(strrep('x', 1e6))", &RunOptions {
        timeout: Some(Duration::from_secs(30)),
        ..Default::default()
    });
    assert_eq!(output.len(), 1_000_000);
}

#[test]
fn test_stderr_is_captured() {
    let output = run_text(
        &find_r(),
        "message('oops'); quit(status = 3)",
        &RunOptions {
            timeout: Some(Duration::from_secs(30)),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(output.status.code(), Some(3));
    // Compare lines because R on Windows writes `\r\n` to a redirected stderr
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(stderr.lines().collect::<Vec<_>>(), vec!["oops"]);
}

/// The asynchronously launched process outlives its parent shell. Killing only
/// the R child would leave it running, but it remains in the child's process group.
#[cfg(unix)]
#[test]
fn test_timeout_kills_grandchild() {
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    let dir = tempfile::tempdir().unwrap();
    let pidfile = dir.path().join("pid");
    let pidfile_str = pidfile.to_str().unwrap();

    let script = r#"
pidfile <- Sys.getenv("OAK_PIDFILE")
system("sh -c 'echo $$ > \"$OAK_PIDFILE\"; exec sleep 60'", wait = FALSE)
while (!file.exists(pidfile)) Sys.sleep(0.05)
Sys.sleep(60)
"#;

    let err = run_text(&find_r(), script, &RunOptions {
        timeout: Some(Duration::from_secs(5)),
        env: &[("OAK_PIDFILE", pidfile_str)],
        ..Default::default()
    })
    .unwrap_err();
    assert_eq!(err.to_string(), "R subprocess timed out after 5s");

    let pid: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let pid = Pid::from_raw(pid);

    // Wait for the orphan to be reaped. Until then, `kill()` with no signal can
    // still find its PID even though the process has exited.
    let deadline = Instant::now() + Duration::from_secs(5);
    while kill(pid, None).is_ok() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(kill(pid, None), Err(Errno::ESRCH));
}

#[test]
fn test_working_dir() {
    let dir = tempfile::tempdir().unwrap();

    let output = run_stdout("cat(getwd())", &RunOptions {
        working_dir: Some(dir.path()),
        ..Default::default()
    });

    assert_eq!(
        std::fs::canonicalize(output).unwrap(),
        std::fs::canonicalize(dir.path()).unwrap()
    );
}

#[test]
fn test_startup_profile_runs_rprofile() {
    // CI sets `R_PROFILE_USER`, which takes precedence over a `.Rprofile` in the
    // working directory
    std::env::remove_var("R_PROFILE_USER");

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".Rprofile"),
        "options(oak_test_profile = TRUE)\n",
    )
    .unwrap();

    let script = "cat(isTRUE(getOption('oak_test_profile')))";

    let output = run_stdout(script, &RunOptions {
        profile: Profile::Startup,
        working_dir: Some(dir.path()),
        ..Default::default()
    });
    assert_eq!(output, "TRUE");

    let output = run_stdout(script, &RunOptions {
        profile: Profile::Vanilla,
        working_dir: Some(dir.path()),
        ..Default::default()
    });
    assert_eq!(output, "FALSE");
}

#[test]
fn test_stdin_is_null() {
    // `readLines()` should see EOF, not wait on an open stdin for input.
    let output = run_stdout("cat(length(readLines(file('stdin'))))", &RunOptions {
        timeout: Some(Duration::from_secs(30)),
        ..Default::default()
    });
    assert_eq!(output, "0");
}

#[test]
fn test_startup_environment_hides_later_variables() {
    capture_startup_environment();
    std::env::set_var("OAK_R_PROCESS_LATE", "late");

    let output = run_stdout(
        "cat(Sys.getenv('OAK_R_PROCESS_LATE', 'unset'), Sys.getenv('OAK_R_PROCESS_CALLER', 'unset'))",
        &RunOptions {
            env: &[("OAK_R_PROCESS_CALLER", "caller")],
            ..Default::default()
        },
    );
    assert_eq!(output, "unset caller");
}

#[test]
fn test_uncaptured_startup_environment_inherits() {
    std::env::set_var("OAK_R_PROCESS_LATE", "late");

    let output = run_stdout(
        "cat(Sys.getenv('OAK_R_PROCESS_LATE', 'unset'))",
        &RunOptions::default(),
    );
    assert_eq!(output, "late");
}
