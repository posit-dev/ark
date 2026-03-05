#
# eval.R
#
# Copyright (C) 2026 Posit Software, PBC. All rights reserved.
#
#

initialize_eval <- function() {
    base_bind(as.symbol(".ark_eval_log_message"), .ark_eval_log_message)
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
