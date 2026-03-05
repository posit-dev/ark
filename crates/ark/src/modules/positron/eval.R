#
# eval.R
#
# Copyright (C) 2026 Posit Software, PBC. All rights reserved.
#
#

initialize_eval <- function() {
    base_bind(as.symbol(".ark_eval_capture"), .ark_eval_capture)
    base_bind(as.symbol(".ark_eval_log_message"), .ark_eval_log_message)
}

# Evaluate an expression while capturing warnings and messages that R
# would otherwise defer or send to the console. Returns
# `list(result, conditions)` where `result` is the raw evaluation result
# and `conditions` is a list of condition objects (inheriting from
# `warning` or `message`) in the order they were signalled.
#' @export
.ark_eval_capture <- function(expr) {
    conditions <- list()

    result <- withCallingHandlers(
        expr,
        warning = function(w) {
            conditions[[length(conditions) + 1L]] <<- w
            invokeRestart("muffleWarning")
        },
        message = function(m) {
            conditions[[length(conditions) + 1L]] <<- m
            invokeRestart("muffleMessage")
        }
    )

    list(result, conditions)
}

# Interpolate a DAP log message template using `glue`. We call
# unconditionally so that a missing glue package surfaces as an
# actionable error rather than silently returning the raw template.
#' @export
.ark_eval_log_message <- function(template, env = parent.frame()) {
    if (!grepl("{", template, fixed = TRUE)) {
        return(template)
    }
    as.character(glue::glue(template, .envir = env))
}
