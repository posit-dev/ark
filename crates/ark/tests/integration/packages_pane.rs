//
// packages_pane.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use ark::modules::ARK_ENVS;
use ark::r_task::r_task;

// R helpers that stand up a fake library and a fake repository so
// `.ps.rpc.pkg_outdated()` can run against real `old.packages()` output on
// whichever R version we are testing against.
//
// Nothing is installed or compiled: `installed.packages()` only reads
// `<lib>/<pkg>/Meta/package.rds`, and `available.packages()` only reads a DCF
// `PACKAGES` file under `<repo>/src/contrib`. `Built` is only a standard field
// for binary repositories, so we ask for it explicitly through
// `available_packages_fields` and the fixture stays a portable source repo.
const FAKE_LIBRARY: &str = r#"
    pkg <- function(name, version, built) {
        list(name = name, version = version, built = built)
    }

    with_fake_library <- function(installed, available, expr) {
        lib <- tempfile("arklib")
        contrib <- file.path(tempfile("arkrepo"), "src", "contrib")
        dir.create(contrib, recursive = TRUE)

        # R >= 4.6 reads the whole `Built` string from the installed
        # DESCRIPTION, older versions read `Built$R`, so supply both.
        for (entry in installed) {
            dir.create(file.path(lib, entry$name, "Meta"), recursive = TRUE)
            saveRDS(
                list(
                    DESCRIPTION = c(
                        Package = entry$name,
                        Version = entry$version,
                        Priority = NA_character_,
                        Built = entry$built
                    ),
                    Built = list(
                        R = package_version(sub("^R ([0-9.]+).*", "\\1", entry$built))
                    )
                ),
                file.path(lib, entry$name, "Meta", "package.rds")
            )
        }

        stanzas <- vapply(
            available,
            function(entry) {
                paste0(
                    "Package: ", entry$name, "\n",
                    "Version: ", entry$version, "\n",
                    "Built: ", entry$built, "\n"
                )
            },
            character(1)
        )
        writeLines(stanzas, file.path(contrib, "PACKAGES"))

        repo <- dirname(dirname(contrib))
        old_libs <- .libPaths()
        old_opts <- options(
            repos = c(CRAN = paste0("file:///", normalizePath(repo, winslash = "/"))),
            pkgType = "source",
            available_packages_fields = "Built"
        )
        on.exit({
            options(old_opts)
            .libPaths(old_libs)
        })
        .libPaths(lib)

        force(expr)
    }

    # A package built by an older R, and the same package rebuilt more recently.
    built_old <- "R 4.4.1; ; 2026-01-01 00:00:00 UTC; unix"
    built_new <- "R 4.6.0; ; 2026-06-01 00:00:00 UTC; unix"

    entries <- function(outdated) {
        vapply(
            outdated,
            function(entry) paste0(entry$name, "@", entry$latestVersion),
            character(1)
        )
    }
"#;

#[test]
fn test_pkg_outdated_reports_only_newer_versions() {
    // `arkfakenumeric` also pins that versions compare as numbers rather than
    // strings: "10.0.0" is newer than "9.0.0" even though it sorts before it.
    let result = eval_in_fake_library(
        r#"
        with_fake_library(
            installed = list(
                pkg("arkfakecurrent", "3.0.0", built_old),
                pkg("arkfakenumeric", "9.0.0", built_old),
                pkg("arkfakerebuild", "1.5.0", built_old),
                pkg("arkfakeupgrade", "1.0.0", built_old)
            ),
            available = list(
                pkg("arkfakecurrent", "3.0.0", built_old),
                pkg("arkfakenumeric", "10.0.0", built_new),
                pkg("arkfakerebuild", "1.5.0", built_new),
                pkg("arkfakeupgrade", "2.0.0", built_new)
            ),
            {
                outdated <- utils::old.packages()
                c(
                    paste(entries(.ps.rpc.pkg_outdated()), collapse = ","),
                    as.character("arkfakerebuild" %in% outdated[, "Package"]),
                    as.character(getRversion() >= "4.6.0")
                )
            }
        )
        "#,
    );

    let [outdated, rebuild_flagged, r_reports_rebuilds] = result.as_slice() else {
        panic!("Expected three elements, got {result:?}");
    };

    assert_eq!(outdated, "arkfakenumeric@10.0.0,arkfakeupgrade@2.0.0");

    // `arkfakerebuild` exists to be filtered out, so check that `old.packages()`
    // really does report it on the R versions that report same-version
    // rebuilds. Otherwise this test could pass while exercising nothing.
    assert_eq!(rebuild_flagged, r_reports_rebuilds);
}

#[test]
fn test_pkg_outdated_empty_when_no_newer_versions() {
    let result = eval_in_fake_library(
        r#"
        with_fake_library(
            installed = list(
                pkg("arkfakecurrent", "3.0.0", built_old),
                pkg("arkfakerebuild", "1.5.0", built_old)
            ),
            available = list(
                pkg("arkfakecurrent", "3.0.0", built_old),
                pkg("arkfakerebuild", "1.5.0", built_new)
            ),
            {
                outdated <- utils::old.packages()
                c(
                    paste(entries(.ps.rpc.pkg_outdated()), collapse = ","),
                    as.character(
                        !is.null(outdated) && "arkfakerebuild" %in% outdated[, "Package"]
                    ),
                    as.character(getRversion() >= "4.6.0"),
                    # `old.packages()` returns `NULL` rather than an empty matrix
                    # when it finds nothing, which is what R < 4.6 does here.
                    paste(entries(pkg_outdated_result(NULL)), collapse = ",")
                )
            }
        )
        "#,
    );

    let [outdated, rebuild_flagged, r_reports_rebuilds, null_input] = result.as_slice() else {
        panic!("Expected four elements, got {result:?}");
    };

    assert_eq!(outdated, "");
    assert_eq!(rebuild_flagged, r_reports_rebuilds);
    assert_eq!(null_input, "");
}

fn eval_in_fake_library(body: &str) -> Vec<String> {
    // Evaluate inside `local()` so the fixture helpers land in a fresh child
    // environment: the positron namespace itself is locked, so assigning into
    // it directly would error.
    let code = format!("local({{\n{FAKE_LIBRARY}\n{body}\n}})");
    r_task(|| {
        harp::parse_eval0(&code, ARK_ENVS.positron_ns)
            .unwrap()
            .try_into()
            .unwrap()
    })
}
