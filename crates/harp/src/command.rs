//
// command.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use std::io;
use std::process::Command;
use std::process::Output;

use crate::sys::command::COMMAND_R_LOCATIONS;

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
    let n = COMMAND_R_LOCATIONS.len();
    assert!(n > 0);

    for (i, program) in COMMAND_R_LOCATIONS.iter().enumerate() {
        // Build the `Command` from the user's function
        let mut command = Command::new(program);
        build(&mut command);

        // Run it, waiting on it to finish
        let out = command.output();

        if out.is_ok() {
            // Found R, executed command successfully
            return out;
        }
        if i == n - 1 {
            // On last location, but still couldn't find R, return error
            return out;
        }
    }

    unreachable!("`assert!` ensures at least 1 program location is provided.");
}
