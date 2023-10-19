#
# r_data_viewer.R
#
# Copyright (C) 2023 Posit Software, PBC. All rights reserved.
#
#

.ps.view_data_frame <- function(x, title) {
    if (missing(title)) {
        title <- paste(deparse(substitute(x), backtick=TRUE), collapse = "")
    }
    stopifnot(
        is.data.frame(x) || is.matrix(x),
        is.character(title) && length(title) == 1L && !is.na(title)
    )
    invisible(.ps.Call("ps_view_data_frame", x, title))
}

.ps.register_utils_hook <- function(name, hook) {
    packageName <- "package:utils"
    original <- base::get(name, packageName, mode="function")
    # check if function exists
    if (is.null(original)) {
        fmt <- "internal error: function utils::%s not found"
        msg <- sprintf(fmt, shQuote(name))
        stop(msg, call. = FALSE)
    }
    utilsEnv <- as.environment(packageName)
    .ps.binding.replace(name, hook, utilsEnv)
}

.ps.register_utils_hook("View", .ps.view_data_frame)
