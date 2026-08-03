//
// env.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

pub fn env_flag(name: &str) -> bool {
    env_flag_opt(name).unwrap_or(false)
}

/// Returns `None` for unset or unrecognized values so callers preserve a
/// lower-precedence setting.
pub fn env_flag_opt(name: &str) -> Option<bool> {
    match std::env::var(name) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" => Some(true),
            "0" | "false" => Some(false),
            _ => None,
        },
        Err(_) => None,
    }
}

pub fn is_ci() -> bool {
    match env_flag_opt("CI") {
        Some(on) => on,
        // providers set `CI` to their own name as well as to a truthy value.
        // Only treat empty values as non-CI.
        None => std::env::var("CI").is_ok_and(|value| !value.trim().is_empty()),
    }
}

#[cfg(test)]
mod tests {
    use super::env_flag;
    use super::env_flag_opt;
    use super::is_ci;

    /// Preserve the distinction between an absent override and an explicit
    /// false override.
    #[test]
    fn test_env_flag_opt_separates_unset_from_false() {
        let name = "STDEXT_TEST_ENV_FLAG_OPT";

        unsafe { std::env::remove_var(name) };
        assert_eq!(env_flag_opt(name), None);

        for value in ["1", "true", "TRUE", " true "] {
            unsafe { std::env::set_var(name, value) };
            assert_eq!(env_flag_opt(name), Some(true));
        }

        for value in ["0", "false", "FALSE", " false "] {
            unsafe { std::env::set_var(name, value) };
            assert_eq!(env_flag_opt(name), Some(false));
        }

        for value in ["", "  ", "yes", "no", "2"] {
            unsafe { std::env::set_var(name, value) };
            assert_eq!(env_flag_opt(name), None);
        }

        unsafe { std::env::remove_var(name) };
    }

    /// A provider naming itself in `CI`, such as Drone or Woodpecker, still
    /// counts as CI.
    #[test]
    fn test_is_ci_treats_any_non_empty_value_as_ci() {
        for value in ["1", "true", "TRUE", "yes", "drone", "woodpecker"] {
            unsafe { std::env::set_var("CI", value) };
            assert!(is_ci());
        }

        for value in ["0", "false", "FALSE", "", "  "] {
            unsafe { std::env::set_var("CI", value) };
            assert!(!is_ci());
        }

        unsafe { std::env::remove_var("CI") };
        assert!(!is_ci());
    }

    #[test]
    fn test_env_flag_only_accepts_1_and_true() {
        let name = "STDEXT_TEST_ENV_FLAG";

        for value in ["1", "true", "TRUE", "True", " true "] {
            unsafe { std::env::set_var(name, value) };
            assert!(env_flag(name));
        }

        for value in ["", "  ", "0", "false", "FALSE", "no", "off", "2", "yes"] {
            unsafe { std::env::set_var(name, value) };
            assert!(!env_flag(name));
        }

        unsafe { std::env::remove_var(name) };
        assert!(!env_flag(name));
    }
}
