//
// command.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use std::io;
use std::path::PathBuf;
use std::process::Command;
use std::process::Output;

use crate::sys::command::COMMAND_R_NAMES;

/// Execute a `Command` for R, trying multiple names for the R executable
///
/// - For unix, this look at `R`
/// - For Windows, this looks at `R` (`R.exe`) and `R.bat` (for rig compatibility)
///
/// The executable name is joined to the path in `R_HOME`. If not set, this is a
/// panic. Use `r_home_setup()` to set `R_HOME` from the R on the `PATH` or use
/// `r_command_from_path()` to exectute R from `PATH` directly.
///
/// Returns the `Ok()` value of the first success, or the `Err()` value of the
/// last failure if all locations fail.
pub fn r_command<F>(build: F) -> io::Result<Output>
where
    F: Fn(&mut Command),
{
    assert!(COMMAND_R_NAMES.len() > 0);

    // Safety: Caller must ensure `R_HOME` is defined. That's usually the case
    // once Ark has properly started.
    let r_home = std::env::var("R_HOME").unwrap();

    let locations: Vec<PathBuf> = COMMAND_R_NAMES
        .map(|loc| std::path::Path::new(&r_home).join("bin").join(loc))
        .into();

    r_command_from_locs(locations, build)
}

/// Use this before calling `r_command()` to ensure that `R_HOME` is set consistently
pub fn r_home_setup() -> PathBuf {
    match std::env::var("R_HOME") {
        Ok(home) => {
            // Get `R_HOME` from env var, typically set by Positron / CI / kernel specification
            PathBuf::from(home)
        },
        Err(_) => {
            // Get `R_HOME` from `PATH`, via `R`
            let Ok(result) = r_command_from_path(|command| {
                command.arg("RHOME");
            }) else {
                panic!("Can't find R or `R_HOME`");
            };

            let r_home = String::from_utf8(result.stdout).unwrap();
            let r_home = r_home.trim();

            // Now set `R_HOME`. From now on, `r_command()` can be used to
            // run exactly the same R as is running in Ark.
            unsafe { std::env::set_var("R_HOME", r_home) };
            PathBuf::from(r_home)
        },
    }
}

/// Execute a `Command` for R found on the `PATH`
///
/// This is like `r_command()` but doesn't assume `R_HOME` is defined.
/// Instead, the R executable is executed as a bare name and the shell
/// executes it from `PATH`.
pub fn r_command_from_path<F>(build: F) -> io::Result<Output>
where
    F: Fn(&mut Command),
{
    assert!(COMMAND_R_NAMES.len() > 0);

    // Use the bare command names so they are found from the `PATH`
    let locations: Vec<PathBuf> = COMMAND_R_NAMES
        .map(|loc| std::path::Path::new(loc).to_path_buf())
        .into();

    r_command_from_locs(locations, build)
}

fn r_command_from_locs<F>(locations: Vec<PathBuf>, build: F) -> io::Result<Output>
where
    F: Fn(&mut Command),
{
    let mut out = None;

    for program in locations.into_iter() {
        // Build the `Command` from the user's function
        let mut command = Command::new(program);
        build(&mut command);

        // Run it, waiting on it to finish.
        // Store it as `out` no matter what. If all locations fail
        // we end up returning the last failure.
        let result = command.output();
        let ok = result.is_ok();
        out = Some(result);

        if ok {
            // We had a successful command, don't try any more
            break;
        }
    }

    // Unwrap: The `assert!` above ensures at least 1 program location is provided
    out.unwrap()
}
