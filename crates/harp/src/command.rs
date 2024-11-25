//
// command.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use std::io;
use std::process::Command;
use std::process::Output;

use crate::sys::command::COMMAND_R_NAMES;

/// Execute a `Command` for R, trying multiple locations where R might exist
///
/// - For unix, this look at `R`
/// - For Windows, this looks at `R` (`R.exe`) and `R.bat` (for rig compatibility)
///
/// Returns the `Ok()` value of the first success, or the `Err()` value of the
/// last failure if all locations fail.
pub fn r_command<F>(build: F) -> io::Result<Output>
where
    F: Fn(&mut Command),
{
    assert!(COMMAND_R_NAMES.len() > 0);

    // If `R_HOME` is defined use that
    let locations = COMMAND_R_NAMES.map(|loc| {
        if let Ok(r_home) = std::env::var("R_HOME") {
            std::path::Path::new(&r_home).join("bin").join(loc)
        } else {
            std::path::Path::new(loc).to_path_buf()
        }
    });

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

    // SAFETY: The `assert!` above ensures at least 1 program location is provided
    out.unwrap()
}
