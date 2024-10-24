//
// envvar.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use crate::exec::RFunction;
use crate::exec::RFunctionExt;

/// Set an environment variable through `Sys.setenv()`
///
/// This is a safe alternative to [std::env::set_var()]. On Windows in particular,
/// using [std::env::set_var()] after R has started up (i.e. after Ark opens the
/// R DLL and calls `setup_Rmainloop()`) has no effect. We believe this is because
/// [std::env::set_var()] ends up calling `SetEnvironmentVariableW()` to set the
/// environment variable, which only affects the Windows API environment space and
/// not the C environment space that R seems to have access to after it has started
/// up. This matters because R's `Sys.getenv()` uses the C level `getenv()` to
/// access environment variables, which only looks at C environment space.
///
/// To affect the C environment space, [std::env::set_var()] would have had to call
/// [libc::putenv()], but that has issues with thread safety, and we have seen this
/// crash R before, so we'd like to stay away from using that ourselves.
///
/// The easiest solution to this problem is to just go through R's `Sys.setenv()`
/// to ensure that R picks up the environment variable update. This also calls
/// `putenv()`, but effectively allows us to use the R thread to synchronise all
/// writes to environment variables.
///
/// If R has not started up yet, you should be safe to call [std::env::set_var()].
/// For example, we do this for `R_HOME` during the startup process and for
/// `R_PROFILE_USER` in some tests. We aren't sure how, but at R startup time
/// the Windows API environment space seems to get synchronized once with the
/// C environment space, which is what allows this to work.
pub fn set_var(key: &str, value: &str) {
    RFunction::new("base", "Sys.setenv")
        .param(key, value)
        .call()
        .unwrap();
}

/// Fetch an environment variable using `Sys.getenv()`
pub fn var(key: &str) -> Option<String> {
    let out = RFunction::new("base", "Sys.getenv")
        .add(key)
        .call()
        .unwrap();

    // Panic: Input is length 1 string, so output must be a length 1 string.
    let out = String::try_from(out).unwrap();

    // If the output is `""`, then the environment variable was unset.
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Remove an environment variable using `Sys.unsetenv()`
pub fn remove_var(key: &str) {
    RFunction::new("base", "Sys.unsetenv")
        .add(key)
        .call()
        .unwrap();
}

#[cfg(test)]
mod tests {
    use crate::envvar::remove_var;
    use crate::envvar::set_var;
    use crate::envvar::var;

    #[test]
    fn test_env() {
        crate::r_task(|| {
            assert_eq!(var("TEST_VAR"), None);

            set_var("TEST_VAR", "VALUE");
            assert_eq!(var("TEST_VAR"), Some(String::from("VALUE")));

            remove_var("TEST_VAR");
            assert_eq!(var("TEST_VAR"), None);
        })
    }
}
