#
# object_explorer.R
#
# Copyright (C) 2026 Posit Software, PBC. All rights reserved.
#
#

view_object <- function(x, title, var, env, accessor) {
    stopifnot(is_explorable_object(x))
    invisible(.ps.Call("ps_view_object", x, title, var, env, accessor))
}

# Lists (other than data frames), environments, S4 objects, pairlists, and
# expression vectors open in the Object Explorer.
is_explorable_object <- function(x) {
    !is.null(x) &&
        !is.data.frame(x) &&
        !is.matrix(x) &&
        (is.list(x) || is.environment(x) || isS4(x) || is.expression(x))
}

#' @export
.ps.object_explorer.format_value <- function(x) {
    paste(utils::capture.output(print(x)), collapse = "\n")
}
