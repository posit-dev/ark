//
// call.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use harp::error::Error;
use harp::eval::RParseEvalOptions;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::utils::r_is_function;

use crate::console;
use crate::lsp::completions::completion_item::completion_item_from_parameter;
use crate::lsp::completions::request::CallDispatch;
use crate::lsp::completions::request::CallRequest;
use crate::lsp::completions::request::SlotReply;
use crate::lsp::completions::sources::utils::set_sort_text_by_first_appearance;

/// Formal names of the evaluated callee, with S3 dispatch on the pipe root or
/// the first argument. Declines when the callee cannot be evaluated, preserving
/// workspace formals. A non-function callee fills the slot with no items,
/// suppressing workspace formals.
///
/// `pipe_root` is the evaluated pipe root, `None` if there is none or it
/// failed to evaluate.
pub(super) fn r_call_completions(
    request: &CallRequest,
    pipe_root: Option<&RObject>,
) -> anyhow::Result<SlotReply> {
    let CallRequest {
        callee,
        dispatch,
        position,
    } = request;

    log::trace!("r_call_completions({callee:?})");

    // `NULL` makes `.ps.completions.formalNames()` use the callee's own formals
    // without S3 dispatch.
    let object = match dispatch {
        CallDispatch::PipeRoot => pipe_root.cloned(),
        CallDispatch::FirstArgument(argument) => eval_first_argument(argument),
        CallDispatch::None => None,
    };
    let object = object.unwrap_or_else(RObject::null);

    let r_callable = harp::parse_eval(callee, RParseEvalOptions {
        forbid_function_calls: true,
        env: console::selected_env(),
    });

    let r_callable = match r_callable {
        Ok(r_callable) => r_callable,
        Err(err) => match err {
            // The function-call guard rejects callee text containing `(`.
            Error::UnsafeEvaluationError(_) => return Ok(SlotReply::Decline),
            // Unfinished code or an unloaded package can leave the callee
            // undefined. Keep workspace formals rather than report an error.
            _ => return Ok(SlotReply::Decline),
        },
    };

    let mut completions = vec![];

    if !r_is_function(r_callable.sexp) {
        // A session non-function shadows the workspace function, so its
        // workspace formals must not be offered.
        return Ok(SlotReply::Fill(completions));
    }

    let strings = RFunction::from(".ps.completions.formalNames")
        .add(r_callable)
        .add(object)
        .call()?
        .to::<Vec<String>>()?;

    for string in strings.iter() {
        match completion_item_from_parameter(string, callee, *position) {
            Ok(item) => completions.push(item),
            Err(err) => log::error!("{err:?}"),
        }
    }

    // Keep signature order rather than sorting argument names alphabetically.
    set_sort_text_by_first_appearance(&mut completions);

    Ok(SlotReply::Fill(completions))
}

fn eval_first_argument(text: &str) -> Option<RObject> {
    let options = RParseEvalOptions {
        forbid_function_calls: true,
        env: console::selected_env(),
    };

    // The function-call guard rejects text containing `(` without logging.
    // R errors, such as an undefined argument in unfinished code, are traced
    // without a backtrace. Other failures are logged as errors.
    match harp::parse_eval(text, options) {
        Ok(value) => Some(value),
        Err(err) => match err {
            Error::UnsafeEvaluationError(_) => None,
            Error::TryCatchError(err) => {
                log::trace!("Can't evaluate first argument: {}", err.message);
                None
            },
            _ => {
                log::error!("Can't evaluate first argument: {err}");
                None
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use harp::eval::RParseEvalOptions;
    use tower_lsp_server::ls_types::CompletionItem;
    use tower_lsp_server::ls_types::CompletionTextEdit;
    use tower_lsp_server::ls_types::Position;
    use tower_lsp_server::ls_types::Range;
    use tower_lsp_server::ls_types::TextEdit;

    use crate::lsp::completions::r_completions::call::r_call_completions;
    use crate::lsp::completions::request::SlotReply;
    use crate::lsp::completions::tests::utils::request_at_cursor;
    use crate::r_task;

    fn call_reply_at_cursor(code_with_cursor: &str) -> SlotReply {
        let request = request_at_cursor(code_with_cursor).composite.call.unwrap();
        r_call_completions(&request, None).unwrap()
    }

    fn call_completions_at_cursor(code_with_cursor: &str) -> Vec<CompletionItem> {
        match call_reply_at_cursor(code_with_cursor) {
            SlotReply::Fill(items) => items,
            reply => panic!("Expected call completions at `{code_with_cursor}`, got {reply:?}"),
        }
    }

    fn labels(completions: &[CompletionItem]) -> Vec<&str> {
        completions.iter().map(|item| item.label.as_str()).collect()
    }

    #[test]
    fn test_completions_after_user_types_part_of_an_argument_name() {
        r_task(|| {
            let completions = call_completions_at_cursor("match(tab@)");

            assert_eq!(completions.len(), 4);
            assert_eq!(completions.first().unwrap().label, "x = ");
            assert_eq!(completions.get(1).unwrap().label, "table = ");

            let completions = call_completions_at_cursor("match(1, tab@)");

            // TODO: Do not offer `x` when the first argument is already supplied.
            assert_eq!(completions.len(), 4);
            assert_eq!(completions.first().unwrap().label, "x = ");
            assert_eq!(completions.get(1).unwrap().label, "table = ");
        })
    }

    #[test]
    fn test_session_arguments() {
        r_task(|| {
            let reply = call_reply_at_cursor("not_a_known_function(@)");
            assert_eq!(reply, SlotReply::Decline);
        });

        r_task(|| {
            let options = RParseEvalOptions {
                forbid_function_calls: false,
                ..Default::default()
            };

            harp::parse_eval("my_fun <- function(y, x) x + y", options.clone()).unwrap();

            let completions = call_completions_at_cursor("my_fun(@)");
            assert_eq!(labels(&completions), vec!["y = ", "x = "]);

            harp::parse_eval("remove(my_fun)", options.clone()).unwrap();
        });

        r_task(|| {
            let options = RParseEvalOptions {
                forbid_function_calls: false,
                ..Default::default()
            };

            harp::parse_eval("my_fun <- 1", options.clone()).unwrap();

            let reply = call_reply_at_cursor("my_fun(@)");
            assert_eq!(reply, SlotReply::Fill(vec![]));

            harp::parse_eval("remove(my_fun)", options.clone()).unwrap();
        })
    }

    #[test]
    fn test_dot_dot_dot_item_is_inserted_empty_at_cursor() {
        r_task(|| {
            let completions = call_completions_at_cursor("paste(@)");
            let item = completions.iter().find(|item| item.label == "...").unwrap();

            let position = Position::new(0, 6);
            assert_eq!(
                item.text_edit,
                Some(CompletionTextEdit::Edit(TextEdit {
                    range: Range::new(position, position),
                    new_text: String::new(),
                }))
            );
        })
    }

    #[test]
    fn test_completions_multiline_call() {
        r_task(|| {
            let completions = call_completions_at_cursor("match(\n  @\n)");

            assert_eq!(completions.len(), 4);
            assert_eq!(completions.first().unwrap().label, "x = ");
            assert_eq!(completions.get(1).unwrap().label, "table = ");

            let completions = call_completions_at_cursor("match(\n  tab@\n)");

            assert_eq!(completions.len(), 4);
            assert_eq!(completions.first().unwrap().label, "x = ");
            assert_eq!(completions.get(1).unwrap().label, "table = ");

            let completions = call_completions_at_cursor("match(\n  1,\n  tab@\n)");

            assert_eq!(completions.len(), 4);
            assert_eq!(completions.first().unwrap().label, "x = ");
            assert_eq!(completions.get(1).unwrap().label, "table = ");
        })
    }
}
