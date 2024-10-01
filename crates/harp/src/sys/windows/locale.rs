/*
 * locale.rs
 *
 * Copyright (C) 2024 Posit Software, PBC. All rights reserved.
 *
 */

#[cfg(test)]
mod tests {
    use crate::r_task;

    #[test]
    fn test_locale() {
        // These tests assert that we've embedded our Application Manifest file correctly in `build.rs`
        r_task(|| {
            let latin1 = harp::parse_eval_base("l10n_info()$`Latin-1`").unwrap();
            let latin1 = bool::try_from(latin1).unwrap();
            assert!(!latin1);

            let utf8 = harp::parse_eval_base("l10n_info()$`UTF-8`").unwrap();
            let utf8 = bool::try_from(utf8).unwrap();
            assert!(utf8);

            let codepage = harp::parse_eval_base("l10n_info()$codepage").unwrap();
            let codepage = i32::try_from(codepage).unwrap();
            assert_eq!(codepage, 65001);

            let system_codepage = harp::parse_eval_base("l10n_info()$system.codepage").unwrap();
            let system_codepage = i32::try_from(system_codepage).unwrap();
            assert_eq!(system_codepage, 65001);
        })
    }
}
