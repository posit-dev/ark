//
// packages_pane.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use ark::modules::ARK_ENVS;
use ark::r_task::r_task;

// Build a fake `old.packages()` matrix from `(package, installed, repos)` rows,
// run it through the pane's `pkg_outdated_result()` helper, and return the
// resulting entries formatted as `name@latestVersion`.
fn pkg_outdated(rows: &str) -> Vec<String> {
    r_task(|| {
        // Evaluate inside `local()` so the helper bindings land in a fresh
        // child environment: the positron namespace itself is locked, so
        // assigning into it directly would error.
        let code = format!(
            r#"
            local({{
                row <- function(pkg, installed, repos) {{
                    c(pkg, "lib", installed, "4.4.0", repos, "CRAN")
                }}
                m <- rbind({rows})
                colnames(m) <- c(
                    "Package", "LibPath", "Installed", "Built", "ReposVer", "Repository"
                )
                res <- pkg_outdated_result(m)
                vapply(res, function(x) paste0(x$name, "@", x$latestVersion), character(1))
            }})
            "#
        );
        harp::parse_eval0(&code, ARK_ENVS.positron_ns)
            .unwrap()
            .try_into()
            .unwrap()
    })
}

#[test]
fn test_pkg_outdated_keeps_newer_repository_version() {
    let outdated = pkg_outdated(r#"row("pkgA", "1.0.0", "2.0.0")"#);
    assert_eq!(outdated, vec![String::from("pkgA@2.0.0")]);
}

#[test]
fn test_pkg_outdated_drops_same_version_rebuild() {
    // old.packages() flags this because the repository copy has a newer Built
    // or publication date, but the version is unchanged so it is not an update.
    let outdated = pkg_outdated(r#"row("pkgB", "1.5.0", "1.5.0")"#);
    assert!(outdated.is_empty());
}

#[test]
fn test_pkg_outdated_keeps_only_real_upgrades() {
    let outdated = pkg_outdated(
        r#"
        row("pkgA", "1.0.0", "2.0.0"),
        row("pkgB", "1.5.0", "1.5.0"),
        row("pkgC", "0.9.0", "0.9.1")
        "#,
    );
    assert_eq!(outdated, vec![
        String::from("pkgA@2.0.0"),
        String::from("pkgC@0.9.1"),
    ]);
}

#[test]
fn test_pkg_outdated_compares_versions_numerically() {
    // Versions must compare as numbers, not strings: "10.0.0" is newer than
    // "9.0.0" even though it sorts before it as a string. A naive string
    // comparison would wrongly hide this upgrade.
    let outdated = pkg_outdated(r#"row("pkgA", "9.0.0", "10.0.0")"#);
    assert_eq!(outdated, vec![String::from("pkgA@10.0.0")]);
}

#[test]
fn test_pkg_outdated_empty_when_all_rebuilds() {
    let outdated = pkg_outdated(
        r#"
        row("pkgA", "1.0.0", "1.0.0"),
        row("pkgB", "1.5.0", "1.5.0")
        "#,
    );
    assert!(outdated.is_empty());
}

#[test]
fn test_pkg_outdated_handles_null() {
    let outdated: Vec<String> = r_task(|| {
        harp::parse_eval0(
            r#"
            local({
                res <- pkg_outdated_result(NULL)
                vapply(res, function(x) x$name, character(1))
            })
            "#,
            ARK_ENVS.positron_ns,
        )
        .unwrap()
        .try_into()
        .unwrap()
    });
    assert!(outdated.is_empty());
}
