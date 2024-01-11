#
# environment.R
#
# Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
#
#

#' @export
.ps.environment.clipboardFormatDataFrame <- function(x) {
    tf <- tempfile()
    on.exit(unlink(tf))

    write.table(x, sep = "\t", file = tf, col.names = NA)

    readLines(tf)
}

#' @export
.ps.environment.describeCall <- function(expr, width.cutoff = 500L, nlines = -1L) {
    # TODO: take inspiration from .rs.deparse() in rstudio
    deparsed <- deparse(
        expr,
        width.cutoff = width.cutoff,
        nlines       = nlines
    )

    paste(deparsed, collapse = " ")
}
