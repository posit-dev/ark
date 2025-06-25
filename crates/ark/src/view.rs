//
// view.rs
//
// Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
//
//

use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::RObject;

use crate::modules::ARK_ENVS;

pub(crate) fn view(x: &RObject, path: &Vec<String>, env: &RObject) -> anyhow::Result<()> {
    // Currently `view()` only supports identifiers
    let name = if path.len() == 1 {
        path.last().unwrap().clone()
    } else {
        String::from("")
    };

    RFunction::new("", "view")
        .add(x.sexp)
        .param("name", name)
        .param("env", env.sexp)
        .call_in(ARK_ENVS.positron_ns)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    macro_rules! eval_and_snapshot {
        ($source:expr) => {{
            let doc = harp::parse_eval_global($source).unwrap();
            let doc: String = doc.try_into().unwrap();

            // Replace addresses like 0x12345 with 0x*address* for snapshot stability
            let doc = regex::Regex::new(r"0x[0-9a-fA-F]+")
                .unwrap()
                .replace_all(&doc, "0x*address*")
                .to_string();

            // Replace PID by a constant for snapshot stability
            let doc = regex::Regex::new(r"ark:ark-\d+")
                .unwrap()
                .replace_all(&doc, "ark:ark-*pid*")
                .to_string();

            insta::assert_snapshot!(doc);

            // Clean up our `foo` objects
            harp::parse_eval_global("if (exists('foo', inherits = FALSE)) rm(foo)").unwrap();
        }};
    }

    #[test]
    fn test_view_function() {
        crate::r_task(|| {
            eval_and_snapshot!(
                "
                {
                    foo <- function(arg) body
                    .ps.internal(view_function_test(foo, 'foo', globalenv()))
                }"
            );
        });
    }

    #[test]
    fn test_view_function_unknown() {
        crate::r_task(|| {
            eval_and_snapshot!(
                "
                {
                    foo <- function(arg) body
                    .ps.internal(view_function_test(foo, '', globalenv()))
                }"
            );
        });
    }

    #[test]
    fn test_view_function_namespace() {
        // FIXME: Looks like namespace generation doesn't work on Windows
        #[cfg(not(target_os = "windows"))]
        crate::r_task(|| {
            let doc = harp::parse_eval_global(
                "
            {
                .ps.internal(view_function_test(identity, '', globalenv()))
            }",
            )
            .unwrap();
            let doc: String = doc.try_into().unwrap();

            let doc = regex::Regex::new(r"ark:ark-\d+")
                .unwrap()
                .replace_all(&doc, "ark:ark-*pid*")
                .to_string();

            assert!(
                doc.contains("ark:ark-*pid*/namespace/base.R"),
                "doc did not contain expected URI. doc was:\n{}",
                doc
            );
        });
    }

    #[test]
    fn test_view_function_local() {
        crate::r_task(|| {
            eval_and_snapshot!(
                "
                {
                    local({
                        foo <- function(arg) body
                        .ps.internal(view_function_test(foo, 'foo', environment()))
                    })
                }"
            )
        });
    }

    #[test]
    fn test_view_function_trace() {
        crate::r_task(|| {
            eval_and_snapshot!(
                "
                {
                    foo <- function(arg) body
                    trace(foo, identity)
                    .ps.internal(view_function_test(foo, 'foo', globalenv()))
                }"
            )
        });
    }
}
