#
# r_data_viewer.R
#
# Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
#
#

#' @export
.ps.view_data_frame <- function(x, title) {
    if (missing(title)) {
        title <- .ps.as_label(substitute(x))
    }
    stopifnot(
        is.data.frame(x) || is.matrix(x),
        is.character(title) && length(title) == 1L && !is.na(title)
    )
    invisible(.ps.Call("ps_view_data_frame", x, title))
}
