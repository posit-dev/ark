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

is_explorable_object <- function(x) {
    .ps.Call("ps_is_explorable", x)
}

#' @export
.ps.object_explorer.format_value <- function(x) {
    paste(utils::capture.output(print(x)), collapse = "\n")
}
