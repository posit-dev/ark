#
# eval.R
#
# Copyright (C) 2026 Posit Software, PBC. All rights reserved.
#
#

initialize_eval <- function() {
    base_bind(as.symbol(".ark_eval_log_message"), .ark_eval_log_message)
}

# Interpolate a DAP log message template using `glue`.
#' @export
.ark_eval_log_message <- function(template, env = parent.frame()) {
    if (!grepl("{", template, fixed = TRUE)) {
        return(template)
    }
    if (!requireNamespace("glue", quietly = TRUE)) {
        stop(
            "Can't interpolate log message: the `glue` package is required but not installed",
            call. = FALSE
        )
    }
    as.character(glue::glue(template, .envir = env))
}
