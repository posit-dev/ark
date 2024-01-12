#
# console.R
#
# Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
#
#

#' Called from the frontend to set the console width.
#'
#' @param width The new console width.
#' @return The old console width.
#' @export
.ps.rpc.setConsoleWidth <- function(width) {
    oldWidth <- getOption("width")
    options(width = width)
    oldWidth
}
