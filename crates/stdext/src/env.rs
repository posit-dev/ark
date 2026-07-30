//
// env.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

pub fn env_flag(name: &str) -> bool {
    match std::env::var(name) {
        Ok(value) => matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true"),
        Err(_) => false,
    }
}

pub fn is_ci() -> bool {
    env_flag("CI")
}

#[cfg(test)]
mod tests {
    use super::env_flag;

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
