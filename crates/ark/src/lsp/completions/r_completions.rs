//
// r_completions.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

mod custom;
mod extractor;
mod file_path;
mod namespace;
pub(crate) mod object_names;
mod subset;

use anyhow::Context;
use tower_lsp_server::ls_types::CompletionItem;

use crate::lsp;
use crate::lsp::completions::r_completions::custom::r_custom_completions;
use crate::lsp::completions::r_completions::extractor::r_extractor_completions;
use crate::lsp::completions::r_completions::file_path::r_file_path_completions;
use crate::lsp::completions::r_completions::namespace::r_namespace_completions;
use crate::lsp::completions::r_completions::subset::r_string_subset_completions;
use crate::lsp::completions::request::RCompletionReplies;
use crate::lsp::completions::request::RCompletionRequest;
use crate::lsp::completions::request::SlotReply;
use crate::lsp::completions::request::UniqueRequest;

/// Answers `request` using the user's R session. Must run on the R thread.
///
/// A source that returns an error declines its slot, preserving the static
/// fallback without preventing other sources from answering.
pub(crate) fn r_completions(request: RCompletionRequest) -> RCompletionReplies {
    let RCompletionRequest { custom, unique } = request;

    let custom = match custom {
        Some(custom) => decline_on_err("custom", r_custom_completions(&custom)),
        None => SlotReply::Decline,
    };

    // A custom override suppresses the unique source's items, so its R
    // evaluation is unnecessary.
    if matches!(custom, SlotReply::Override(_)) {
        return RCompletionReplies {
            custom,
            unique: SlotReply::Decline,
        };
    }

    let unique = match unique {
        Some(unique) => r_unique_completions(unique),
        None => SlotReply::Decline,
    };

    RCompletionReplies { custom, unique }
}

fn r_unique_completions(request: UniqueRequest) -> SlotReply {
    match request {
        UniqueRequest::StringSubset { object } => {
            fill_or_decline("string_subset", r_string_subset_completions(&object))
        },
        UniqueRequest::FilePath { token } => {
            fill_or_decline("file_path", r_file_path_completions(&token))
        },
        UniqueRequest::Namespace {
            package,
            exports_only,
            function_context,
        } => fill_or_decline(
            "namespace",
            r_namespace_completions(&package, exports_only, &function_context),
        ),
        UniqueRequest::Extractor { object, operator } => {
            fill_or_decline("extractor", r_extractor_completions(&object, operator))
        },
    }
}

fn fill_or_decline(source: &str, items: anyhow::Result<Vec<CompletionItem>>) -> SlotReply {
    decline_on_err(source, items.map(SlotReply::Fill))
}

fn decline_on_err(source: &str, reply: anyhow::Result<SlotReply>) -> SlotReply {
    let err = match reply.with_context(|| format!("Session completion source `{source}` failed")) {
        Ok(reply) => return reply,
        Err(err) => err,
    };

    #[cfg(test)]
    expected_errors::record(source, &err);

    // `lsp::log_error!` reaches only the client, as a `window/logMessage` that
    // the integration test client rejects unless the test calls
    // `allow_log_message()`. Without that, a test whose static items hide the
    // declined slot would pass. `log::error!` keeps the error in the kernel log.
    log::error!("{err:?}");
    lsp::log_error!("{err:?}");

    SlotReply::Decline
}

/// Static fallbacks can hide session errors from tests that only inspect
/// completion items. Panic on those errors unless the test explicitly allows
/// them with [expect_session_errors()].
#[cfg(test)]
pub(crate) mod expected_errors {
    use std::cell::Cell;

    thread_local! {
        static EXPECTED: Cell<Option<usize>> = const { Cell::new(None) };
    }

    /// Runs `f` without panicking on session source errors and returns their
    /// count alongside the result.
    pub(crate) fn expect_session_errors<T>(f: impl FnOnce() -> T) -> (T, usize) {
        let outer = EXPECTED.replace(Some(0));
        let value = f();
        let count = EXPECTED.replace(outer).unwrap_or_default();
        (value, count)
    }

    pub(super) fn record(source: &str, err: &anyhow::Error) {
        let Some(count) = EXPECTED.get() else {
            panic!(
                "Session completion source `{source}` declined with an error. \
                 Run the test under `expect_session_errors()` if it's expected: {err:?}"
            );
        };
        EXPECTED.set(Some(count + 1));
    }
}

#[cfg(test)]
mod tests {
    use harp::exec::RFunction;
    use harp::exec::RFunctionExt;

    use crate::lsp::completions::r_completions::expected_errors::expect_session_errors;
    use crate::lsp::completions::tests::utils::find_completion_by_label;
    use crate::lsp::completions::tests::utils::get_completions_at_cursor;
    use crate::r_task;

    /// See `test_dollar_answers_when_custom_declines()` for the answer
    /// without the session.
    #[test]
    fn test_custom_overrides_dollar() {
        r_task(|| {
            let name = "ARK_TEST_ENVVAR";
            harp::parse_eval_base(format!("Sys.setenv({name} = '1')").as_str()).unwrap();

            let n_envvars = harp::parse_eval_base("length(Sys.getenv())").unwrap();
            let n_envvars = usize::try_from(i32::try_from(n_envvars).unwrap()).unwrap();

            // The custom callback suppresses the `$` slot.
            let completions = get_completions_at_cursor("Sys.getenv(foo$@)").unwrap();
            assert_eq!(completions.len(), n_envvars);
            assert!(find_completion_by_label(&completions, name).is_some());

            harp::parse_eval_base(format!("Sys.unsetenv('{name}')").as_str()).unwrap();
        })
    }

    #[test]
    fn test_custom_error_declines() {
        r_task(|| {
            register_failing_callback();

            let (completions, n_errors) =
                expect_session_errors(|| get_completions_at_cursor("f(arg@)").unwrap());
            assert_eq!(n_errors, 1);

            // The custom error leaves composite call completions intact.
            assert_eq!(completions[0].label, "arg = ");
        })
    }

    #[test]
    #[should_panic(expected = "declined with an error")]
    fn test_undeclared_error_panics() {
        r_task(|| {
            register_failing_callback();

            get_completions_at_cursor("f(arg@)").unwrap();
        })
    }

    /// Defines `f(arg)` and a custom completion callback for `arg` that
    /// raises an R error.
    fn register_failing_callback() {
        harp::parse_eval_global("f <- function(arg) NULL").unwrap();
        let callback = harp::parse_eval_global("function(position) stop('boom')").unwrap();
        RFunction::from(".ps.completions.registerCustomCompletionHandler")
            .add("pkg")
            .add("f")
            .add("arg")
            .add(callback)
            .call()
            .unwrap();
    }
}
