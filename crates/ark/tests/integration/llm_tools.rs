//
// llm_tools.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use ark::modules::ARK_ENVS;
use ark::r_task::r_task;

fn eval_in_positron_ns(code: &str) -> Vec<String> {
    // `local()` keeps assignments out of the locked positron namespace
    let code = format!("local({{\n{code}\n}})");
    r_task(|| {
        harp::parse_eval0(&code, ARK_ENVS.positron_ns)
            .unwrap()
            .try_into()
            .unwrap()
    })
}

#[test]
fn test_help_package_topic_sorts_matches_by_package() {
    let result = eval_in_positron_ns(
        r#"
        help_page <- structure(
            c("/lib/stats/help/filter", "/lib/zoo/help/filter_z", "/lib/dplyr/help/filter_d"),
            topic = "filter"
        )
        resolved <- help_package_topic(help_page)
        paste0(resolved$package, "::", resolved$resolved)
        "#,
    );
    assert_eq!(result, vec![
        "dplyr::filter_d",
        "stats::filter",
        "zoo::filter_z"
    ]);
}

#[test]
fn test_get_package_vignette_returns_source_without_html() {
    // grid's vignettes are rendered to PDF, so the source is returned as is
    let result = eval_in_positron_ns(
        r#"
        vignette <- .ps.rpc.get_package_vignette("grid", "moveline")
        rnw <- system.file("doc", "moveline.Rnw", package = "grid")
        c(
            vignette$name,
            vignette$package,
            as.character(identical(vignette$content, paste(readLines(rnw), collapse = "\n")))
        )
        "#,
    );
    assert_eq!(result, vec!["moveline", "grid", "TRUE"]);
}
