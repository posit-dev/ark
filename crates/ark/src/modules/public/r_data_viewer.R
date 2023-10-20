#
# r_data_viewer.R
#
# Copyright (C) 2023 Posit Software, PBC. All rights reserved.
#
#

.ps.view_data_frame <- function(x, title) {
    if (missing(title)) {
        title <- .ps.as_label(x)
    }
    stopifnot(
        is.data.frame(x) || is.matrix(x),
        is.character(title) && length(title) == 1L && !is.na(title)
    )
    invisible(.ps.Call("ps_view_data_frame", x, title))
}

.ps.register_utils_hook("View", .ps.view_data_frame, namespace = TRUE)
