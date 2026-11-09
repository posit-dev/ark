//
// r_completions.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

mod call;
mod completion_item;
mod custom;
mod extractor;
mod file_path;
mod object_names;
mod pipe;
mod search_path;
mod subset;

use anyhow::Context;
use tower_lsp_server::ls_types::CompletionItem;

use crate::lsp;
use crate::lsp::completions::r_completions::call::r_call_completions;
use crate::lsp::completions::r_completions::custom::r_custom_completions;
use crate::lsp::completions::r_completions::extractor::r_extractor_completions;
use crate::lsp::completions::r_completions::file_path::r_file_path_completions;
use crate::lsp::completions::r_completions::pipe::eval_pipe_root;
use crate::lsp::completions::r_completions::pipe::r_pipe_completions;
use crate::lsp::completions::r_completions::search_path::r_search_path_completions;
use crate::lsp::completions::r_completions::subset::r_string_subset_completions;
use crate::lsp::completions::r_completions::subset::r_subset_completions;
use crate::lsp::completions::request::CompositeReplies;
use crate::lsp::completions::request::CompositeRequest;
use crate::lsp::completions::request::RCompletionReplies;
use crate::lsp::completions::request::RCompletionRequest;
use crate::lsp::completions::request::SlotReply;
use crate::lsp::completions::request::UniqueRequest;

/// Answers `request` using the user's R session. Must run on the R thread.
///
/// A source that returns an error declines its slot, preserving the static
/// fallback without preventing other sources from answering.
pub(crate) fn r_completions(request: RCompletionRequest) -> RCompletionReplies {
    #[cfg(any(test, feature = "testing"))]
    test_hooks::raise_r_error_on_sentinel(&request);

    let RCompletionRequest {
        custom,
        unique,
        composite,
    } = request;

    let custom = match custom {
        Some(custom) => run_slot("custom", || r_custom_completions(&custom)),
        None => SlotReply::Decline,
    };

    // A custom fill discards every other slot's items. Skip their R
    // evaluation to avoid reporting errors from unused completions, such as
    // a failing `names()` method on a pipe root.
    if matches!(custom, SlotReply::Fill(_)) {
        return RCompletionReplies {
            custom,
            ..Default::default()
        };
    }

    let unique = match unique {
        Some(unique) => r_unique_completions(unique),
        None => SlotReply::Decline,
    };

    let composite = r_composite_completions(composite);

    RCompletionReplies {
        custom,
        unique,
        composite,
    }
}

fn r_unique_completions(request: UniqueRequest) -> SlotReply {
    match request {
        UniqueRequest::StringSubset { object } => {
            run_fill_slot("string_subset", || r_string_subset_completions(&object))
        },
        UniqueRequest::FilePath { token } => {
            run_fill_slot("file_path", || r_file_path_completions(&token))
        },
        UniqueRequest::Extractor { object, operator } => {
            run_fill_slot("extractor", || r_extractor_completions(&object, operator))
        },
    }
}

fn r_composite_completions(request: CompositeRequest) -> CompositeReplies {
    let CompositeRequest {
        pipe_root,
        call,
        subset,
        search_path,
    } = request;

    // Share the evaluated root between S3 dispatch and pipe-name completions.
    let pipe_root = pipe_root.map(|name| {
        let object = eval_pipe_root(&name);
        (name, object)
    });
    let pipe_root_object = pipe_root.as_ref().and_then(|(_, object)| object.as_ref());

    let call = match call {
        Some(call) => run_slot("call", || r_call_completions(&call, pipe_root_object)),
        None => SlotReply::Decline,
    };

    let pipe = match &pipe_root {
        Some((name, Some(object))) => run_fill_slot("pipe", || r_pipe_completions(name, object)),
        _ => SlotReply::Decline,
    };

    let subset = match subset {
        Some(subset) => run_fill_slot("subset", || r_subset_completions(&subset)),
        None => SlotReply::Decline,
    };

    let search_path = match search_path {
        Some(search_path) => {
            run_fill_slot("search_path", || r_search_path_completions(&search_path))
        },
        None => SlotReply::Decline,
    };

    CompositeReplies {
        call,
        pipe,
        subset,
        search_path,
    }
}

/// Runs a session source with the info logs that `collect_completions()`
/// writes for static sources, so the kernel log traces every source the
/// request tried and how many items each produced.
fn run_slot(source: &str, reply: impl FnOnce() -> anyhow::Result<SlotReply>) -> SlotReply {
    log::info!("Trying completions from source: {source}");

    let reply = decline_on_err(source, reply());
    if let SlotReply::Fill(items) = &reply {
        log::info!(
            "Found {n} completions from source: {source}",
            n = items.len()
        );
    }

    reply
}

fn run_fill_slot(
    source: &str,
    items: impl FnOnce() -> anyhow::Result<Vec<CompletionItem>>,
) -> SlotReply {
    run_slot(source, || items().map(SlotReply::Fill))
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

/// Completing `.ark_test_completions_r_error$` raises an R error that
/// escapes [r_completions()]. This tests request-level recovery rather than
/// the per-source fallback for errors caught by `harp`.
#[cfg(any(test, feature = "testing"))]
mod test_hooks {
    use crate::lsp::completions::request::RCompletionRequest;
    use crate::lsp::completions::request::UniqueRequest;

    const R_ERROR_OBJECT: &str = ".ark_test_completions_r_error";

    pub(super) fn raise_r_error_on_sentinel(request: &RCompletionRequest) {
        let Some(UniqueRequest::Extractor { object, .. }) = &request.unique else {
            return;
        };
        if object != R_ERROR_OBJECT {
            return;
        }

        // The R longjump skips the request's destructor, leaking its heap
        // allocations. The request owns no database handles or R objects.
        unsafe { libr::Rf_error(c"Test R error in the session completion pass".as_ptr()) }
    }
}

#[cfg(test)]
mod tests {
    use harp::eval::parse_eval_global;
    use harp::exec::RFunction;
    use harp::exec::RFunctionExt;
    use tower_lsp_server::ls_types::CompletionItem;
    use tower_lsp_server::ls_types::CompletionItemKind;
    use tower_lsp_server::ls_types::Documentation;
    use tower_lsp_server::ls_types::MarkupContent;
    use tower_lsp_server::ls_types::MarkupKind;

    use crate::lsp::completions::finish;
    use crate::lsp::completions::r_completions;
    use crate::lsp::completions::r_completions::expected_errors::expect_session_errors;
    use crate::lsp::completions::request::ExtractOperator;
    use crate::lsp::completions::request::RCompletionReplies;
    use crate::lsp::completions::request::SlotReply;
    use crate::lsp::completions::request::UniqueRequest;
    use crate::lsp::completions::tests::utils::find_completion_by_label;
    use crate::lsp::completions::tests::utils::get_completions_at_cursor;
    use crate::lsp::completions::tests::utils::request_at_cursor;
    use crate::lsp::completions::tests::utils::static_completions_at_cursor;
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

    #[test]
    fn test_search_path_and_document_merge() {
        r_task(|| {
            harp::parse_eval_global("fo_both <- 1").unwrap();

            let (completions, request) =
                static_completions_at_cursor("fo_both <- 2\nfo_doc <- 3\nfo@").unwrap();
            let replies = r_completions(request);
            assert!(matches!(replies.composite.search_path, SlotReply::Fill(_)));

            let completions = finish(completions, replies);

            let item = find_completion_by_label(&completions, "format").unwrap();
            assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION));

            let item = find_completion_by_label(&completions, "fo_doc").unwrap();
            assert_eq!(item.kind, Some(CompletionItemKind::VARIABLE));

            // The document's `VARIABLE` replaces the search path's `STRUCT`.
            let items = items_with_label(&completions, "fo_both");
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].kind, Some(CompletionItemKind::VARIABLE));
        })
    }

    #[test]
    fn test_unevaluable_pipe_root_keeps_other_slots() {
        r_task(|| {
            let (completions, request) =
                static_completions_at_cursor("fo_doc <- 1\nark_no_root |> ark_no_fn(fo@)").unwrap();
            assert_eq!(
                request.composite.pipe_root,
                Some(String::from("ark_no_root"))
            );

            let replies = r_completions(request);
            assert_eq!(replies.composite.pipe, SlotReply::Decline);
            assert_eq!(replies.composite.call, SlotReply::Decline);

            let completions = finish(completions, replies);
            assert!(find_completion_by_label(&completions, "format").is_some());
            assert!(find_completion_by_label(&completions, "fo_doc").is_some());
        })
    }

    #[test]
    fn test_composite_error_declines_its_slot() {
        r_task(|| {
            define_failing_pipe_root();
            harp::parse_eval_global("ark_fn <- function(ark_arg) NULL").unwrap();

            let (completions, n_errors) = expect_session_errors(|| {
                get_completions_at_cursor("fo_doc <- 1\nark_root |> ark_fn(fo@)").unwrap()
            });
            assert_eq!(n_errors, 1);

            // The call slot's reply survives the failing pipe slot
            assert!(find_completion_by_label(&completions, "ark_arg = ").is_some());
            assert!(find_completion_by_label(&completions, "format").is_some());
            assert!(find_completion_by_label(&completions, "fo_doc").is_some());
        })
    }

    #[test]
    fn test_custom_override_wins_over_failing_pipe_root() {
        r_task(|| {
            define_failing_pipe_root();
            register_callback(
                "function(position) .ps.completions.createCustomCompletions(c('one', 'two'))",
            );

            // Do not use `expect_session_errors()` here. The custom override
            // must skip the pipe slot, so its failing `names()` method never runs.
            let completions = get_completions_at_cursor("ark_root |> f(arg@)").unwrap();
            assert_eq!(labels(&completions), vec!["one", "two"]);
        })
    }

    #[test]
    fn test_custom_override_skips_composite_slots() {
        r_task(|| {
            harp::parse_eval_global(
                "makeActiveBinding('ark_root', function() { ark_root_evaluated <<- TRUE; list(a = 1) }, globalenv())",
            )
            .unwrap();
            register_callback("function(position) .ps.completions.createCustomCompletions('one')");

            let request = request_at_cursor("ark_root |> f(arg@)");
            assert_eq!(request.composite.pipe_root, Some(String::from("ark_root")));
            assert!(request.composite.call.is_some());
            assert!(request.composite.search_path.is_some());

            let replies = r_completions(request);
            assert!(matches!(replies.custom, SlotReply::Fill(_)));
            assert_eq!(replies.composite.call, SlotReply::Decline);
            assert_eq!(replies.composite.pipe, SlotReply::Decline);
            assert_eq!(replies.composite.search_path, SlotReply::Decline);

            let evaluated = harp::parse_eval_global("exists('ark_root_evaluated')").unwrap();
            assert!(!bool::try_from(evaluated).unwrap());
        })
    }

    #[test]
    fn test_empty_custom_fill_replaces_list() {
        r_task(|| {
            harp::parse_eval_global(
                "makeActiveBinding('ark_root', function() { ark_root_evaluated <<- TRUE; list(a = 1) }, globalenv())",
            )
            .unwrap();
            register_callback(
                "function(position) .ps.completions.createCustomCompletions(character())",
            );

            let code = "ark_root |> f(arg@)";

            // Without the session, the static fallback has items
            let (completions, _) = static_completions_at_cursor(code).unwrap();
            assert!(!finish(completions, RCompletionReplies::default()).is_empty());

            let (completions, request) = static_completions_at_cursor(code).unwrap();
            let replies = r_completions(request);
            assert_eq!(replies.custom, SlotReply::Fill(vec![]));
            assert_eq!(replies.composite.pipe, SlotReply::Decline);
            assert_eq!(finish(completions, replies), vec![]);

            let evaluated = harp::parse_eval_global("exists('ark_root_evaluated')").unwrap();
            assert!(!bool::try_from(evaluated).unwrap());
        })
    }

    #[test]
    fn test_custom_fill_skips_extractor() {
        r_task(|| {
            harp::parse_eval_global(
                "makeActiveBinding('ark_obj', function() { ark_obj_evaluated <<- TRUE; list(a = 1) }, globalenv())",
            )
            .unwrap();
            register_callback("function(position) .ps.completions.createCustomCompletions('one')");

            let request = request_at_cursor("f(ark_obj$@)");
            assert!(request.custom.is_some());
            assert_eq!(
                request.unique,
                Some(UniqueRequest::Extractor {
                    object: String::from("ark_obj"),
                    operator: ExtractOperator::Dollar,
                })
            );

            let replies = r_completions(request);
            assert!(matches!(replies.custom, SlotReply::Fill(_)));
            assert_eq!(replies.unique, SlotReply::Decline);

            let evaluated = harp::parse_eval_global("exists('ark_obj_evaluated')").unwrap();
            assert!(!bool::try_from(evaluated).unwrap());
        })
    }

    #[test]
    fn test_call_error_keeps_static_formals() {
        r_task(|| {
            // S3 dispatch on `ark_obj` looks up `ark_gen.ark_class`, whose
            // active binding makes `.ps.completions.formalNames()` raise
            harp::parse_eval_global("ark_gen <- function(x, ...) UseMethod('ark_gen')").unwrap();
            harp::parse_eval_global("ark_obj <- structure(1, class = 'ark_class')").unwrap();
            harp::parse_eval_global(
                "makeActiveBinding('ark_gen.ark_class', function() stop('boom'), globalenv())",
            )
            .unwrap();

            let (completions, request) = static_completions_at_cursor(
                "ark_gen <- function(static_arg) NULL\nark_gen(ark_obj, @)",
            )
            .unwrap();
            let (replies, n_errors) = expect_session_errors(|| r_completions(request));
            assert_eq!(n_errors, 1);
            assert_eq!(replies.composite.call, SlotReply::Decline);

            let completions = finish(completions, replies);
            assert!(find_completion_by_label(&completions, "static_arg = ").is_some());
        })
    }

    #[test]
    fn test_session_formals_replace_static_formals() {
        r_task(|| {
            harp::parse_eval_global("my_fun <- function(y, x) NULL").unwrap();

            let (completions, request) =
                static_completions_at_cursor("my_fun <- function(a, y) NULL\nmy_fun(@)").unwrap();
            let completions = finish(completions, r_completions(request));

            assert_eq!(labels(&completions), vec!["y = ", "x = "]);
        })
    }

    #[test]
    fn test_non_function_session_binding_keeps_static_formals() {
        r_task(|| {
            // R skips `my_fun <- 1` when it looks up the function of a call,
            // so the session has no function to describe
            harp::parse_eval_global("my_fun <- 1").unwrap();

            let (completions, request) =
                static_completions_at_cursor("my_fun <- function(a, y) NULL\nmy_fun(@)").unwrap();
            let completions = finish(completions, r_completions(request));

            assert_eq!(labels(&completions), vec!["a = ", "y = "]);
        })
    }

    #[test]
    fn test_object_from_document_and_global_env_is_deduped() {
        r_task(|| {
            parse_eval_global("outPath <- 'myDirectory'").unwrap();

            // Mimics retyping in the editor, where every request must give the
            // same item so the icon and position don't change
            for _ in 0..5 {
                let items = completions_with_label("outPath <- 'myDirectory'\nout@", "outPath");
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].kind, Some(CompletionItemKind::VARIABLE));
                assert_eq!(items[0].sort_text, Some("2-outPath".to_string()));
                assert_eq!(items[0].label_details, None);
                assert_eq!(items[0].documentation, Some(defined_on_line_docs(1)));
            }
        });
    }

    #[test]
    fn test_document_object_replaces_shadowed_package_object() {
        r_task(|| {
            // Not run, so only base's `letters` is on the search path
            let items = completions_with_label("letters <- 1:3\nlett@", "letters");
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].kind, Some(CompletionItemKind::VARIABLE));
            assert_eq!(items[0].label_details, None);
            assert_eq!(items[0].documentation, Some(defined_on_line_docs(1)));
        });
    }

    #[test]
    fn test_parameter_replaces_shadowed_package_object() {
        r_task(|| {
            let items = completions_with_label("f <- function(letters) lett@", "letters");
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].kind, Some(CompletionItemKind::VARIABLE));
            assert_eq!(items[0].label_details, None);
        });
    }

    #[test]
    fn test_document_only_object_keeps_variable_kind() {
        r_task(|| {
            let items = completions_with_label("outPath <- 'myDirectory'\nout@", "outPath");
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].kind, Some(CompletionItemKind::VARIABLE));
        });
    }

    #[test]
    fn test_pipe_column_wins_over_global_object() {
        r_task(|| {
            parse_eval_global("df <- data.frame(xcol = 1); xcol <- 1").unwrap();

            let items = completions_with_label("df |> foo(xcol@)", "xcol");
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].kind, Some(CompletionItemKind::VARIABLE));
            assert_eq!(items[0].detail, Some("df".to_string()));
        });
    }

    #[test]
    fn test_subset_column_and_global_object_are_both_kept() {
        r_task(|| {
            parse_eval_global("df <- data.frame(x = 1); x <- 1").unwrap();

            // Subset inserts `"x"`, search path inserts `x`
            let items = completions_with_label("df[x@]", "x");
            assert_eq!(items.len(), 2);
            assert_eq!(items[0].insert_text, Some("\"x\"".to_string()));
            assert_eq!(items[0].detail, Some("df".to_string()));
            assert_eq!(items[1].kind, Some(CompletionItemKind::STRUCT));
            assert_eq!(items[1].insert_text, None);
        });
    }

    #[test]
    fn test_pipe_root_is_evaluated_once() {
        r_task(|| {
            harp::parse_eval_global("ark_root_count <- 0L").unwrap();
            harp::parse_eval_global(
                "makeActiveBinding('ark_root', function() { ark_root_count <<- ark_root_count + 1L; structure(list(col_a = 1, col_b = 2), class = 'ark_test_class') }, globalenv())",
            )
            .unwrap();
            harp::parse_eval_global("f <- function(x, ...) UseMethod('f')").unwrap();
            harp::parse_eval_global("f.ark_test_class <- function(x, method_arg, ...) NULL")
                .unwrap();

            let completions = get_completions_at_cursor("ark_root |> f(@)").unwrap();

            // The call slot dispatches on the root, the pipe slot lists its names
            assert!(find_completion_by_label(&completions, "method_arg = ").is_some());
            assert!(find_completion_by_label(&completions, "col_a").is_some());

            let count = harp::parse_eval_global("ark_root_count").unwrap();
            assert_eq!(i32::try_from(count).unwrap(), 1);

            harp::parse_eval_global("rm(ark_root, ark_root_count, f, f.ark_test_class)").unwrap();
        })
    }

    /// Defines `ark_root`, an object whose `names()` method raises an R error.
    fn define_failing_pipe_root() {
        harp::parse_eval_global(
            "registerS3method('names', 'ark_failing', function(x) stop('boom'))",
        )
        .unwrap();
        harp::parse_eval_global("ark_root <- structure(list(), class = 'ark_failing')").unwrap();
    }

    /// Defines `f(arg)` and a custom completion callback for `arg` that
    /// raises an R error.
    fn register_failing_callback() {
        register_callback("function(position) stop('boom')");
    }

    /// Defines `f(arg)` and registers `callback` as its custom completion
    /// callback for `arg`.
    fn register_callback(callback: &str) {
        harp::parse_eval_global("f <- function(arg) NULL").unwrap();
        let callback = harp::parse_eval_global(callback).unwrap();
        RFunction::from(".ps.completions.registerCustomCompletionHandler")
            .add("pkg")
            .add("f")
            .add("arg")
            .add(callback)
            .call()
            .unwrap();
    }

    fn labels(completions: &[CompletionItem]) -> Vec<&str> {
        completions.iter().map(|item| item.label.as_str()).collect()
    }

    fn items_with_label<'a>(
        completions: &'a [CompletionItem],
        label: &str,
    ) -> Vec<&'a CompletionItem> {
        completions
            .iter()
            .filter(|item| item.label == label)
            .collect()
    }

    fn defined_on_line_docs(line: usize) -> Documentation {
        Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!("Defined in this document on line {line}."),
        })
    }

    fn completions_with_label(code: &str, label: &str) -> Vec<CompletionItem> {
        get_completions_at_cursor(code)
            .unwrap()
            .into_iter()
            .filter(|item| item.label == label)
            .collect()
    }
}
