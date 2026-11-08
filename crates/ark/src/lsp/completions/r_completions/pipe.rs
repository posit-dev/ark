//
// pipe.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use harp::error::Error;
use harp::eval::RParseEvalOptions;
use harp::object::RObject;
use tower_lsp_server::ls_types::CompletionItem;

use crate::console;
use crate::lsp::completions::completion_item::QuoteStyle;
use crate::lsp::completions::r_completions::object_names::completions_from_object_names;

/// Evaluates the pipe root `name` with `forbid_function_calls` enabled.
/// Rejected text or evaluation failure returns `None`, so the pipe slot
/// declines and the call slot uses `NULL` for dispatch rather than falling
/// back to the call's first argument.
pub(super) fn eval_pipe_root(name: &str) -> Option<RObject> {
    let options = RParseEvalOptions {
        forbid_function_calls: true,
        env: console::selected_env(),
    };

    let value = harp::parse_eval(name, options);

    // The function-call guard rejects text containing `(` without logging.
    // R errors, such as an undefined root in unfinished code, are traced
    // without a backtrace. Other failures are logged as errors.
    let value = match value {
        Ok(value) => value,
        Err(err) => match err {
            Error::UnsafeEvaluationError(_) => return None,
            Error::TryCatchError(err) => {
                log::trace!("Can't evaluate pipe root: {}", err.message);
                return None;
            },
            _ => {
                log::error!("Can't evaluate pipe root: {err:?}");
                return None;
            },
        },
    };

    Some(value)
}

/// Names of the evaluated pipe root `object`, such as the columns of a data
/// frame.
pub(super) fn r_pipe_completions(
    name: &str,
    object: &RObject,
) -> anyhow::Result<Vec<CompletionItem>> {
    completions_from_object_names(object.clone(), name, QuoteStyle::BacktickIfNotSyntactic)
}

#[cfg(test)]
mod tests {
    use harp::eval::RParseEvalOptions;

    use crate::lsp::completions::r_completions::pipe::eval_pipe_root;
    use crate::r_task;

    #[test]
    fn test_eval_pipe_root() {
        r_task(|| {
            let options = RParseEvalOptions {
                forbid_function_calls: false,
                ..Default::default()
            };

            assert!(eval_pipe_root("x").is_none());

            harp::parse_eval("x <- data.frame(a = 1)", options.clone()).unwrap();
            assert!(eval_pipe_root("x").is_some());

            harp::parse_eval("remove(x)", options.clone()).unwrap();
        });
    }
}
