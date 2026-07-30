//
// env.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

pub fn env_flag(name: &str) -> bool {
    env_flag_opt(name).unwrap_or(false)
}

/// `None` when unset, or set to anything we don't recognise. Lets a caller fall
/// back to another source of truth rather than to a hardcoded default.
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
    env_flag("CI")
}

#[cfg(test)]
mod tests {
    use super::env_flag;
    use super::env_flag_opt;

    /// `None` has to be distinguishable from `Some(false)`, otherwise an env var
    /// can't override a setting: unset must fall through to the setting, while
    /// `0` must beat it.
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

        // Unrecognised reads as unset, so a typo falls through rather than
        // silently meaning "off".
        for value in ["", "  ", "yes", "no", "2"] {
            unsafe { std::env::set_var(name, value) };
            assert_eq!(env_flag_opt(name), None);
        }

        unsafe { std::env::remove_var(name) };
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
