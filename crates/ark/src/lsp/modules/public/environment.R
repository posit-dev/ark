#
# environment.R
#
# Copyright (C) 2023 Posit Software, PBC. All rights reserved.
#
#

.ps.environment.clipboardFormatDataFrame <- function(x) {
    tf <- tempfile()
    on.exit(unlink(tf))

    write.table(x, sep = "\t", file = tf, col.names = NA)

    readLines(tf)
}

.ps.environment.describeCall <- function(expr, width.cutoff = 500L, nlines = -1L) {
    # TODO: take inspiration from .rs.deparse() in rstudio
    deparse(
        expr,
        width.cutoff = width.cutoff,
        nlines       = nlines
    )
}
