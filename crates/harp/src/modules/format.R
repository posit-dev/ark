#
# format.R
#
# Copyright (C) 2023 Posit Software, PBC. All rights reserved.
#
#

# Works around limitations in `base::format()`. Throws an error if the
# return value of `format()` is not a character vector or does not have the
# same number of dimensions as the input.
harp_format <- function(x, ...) {
    if (is.object(x) && is_matrix(x)) {
        format_oo_matrix(x, ...)
    } else {
        base::format(x, ...)
    }
}

format_oo_matrix <- function(x, ...) {
    out <- base::format(x, ...)

    if (!is.character(out)) {
        stop(sprintf(
            "`format()` method for <%s> must return a character vector.",
            class_collapsed(x)
        ))
    }

    # Try to recover if dimensions don't agree (for example `format.Surv()`
    # doesn't preserve dimensions, see https://github.com/posit-dev/positron/issues/1862)
    if (!identical(dim(x), dim(out))) {
        if (length(x) != length(out)) {
            stop(
                "`format()` method for <%s> must return the same number of elements.",
                class_collapsed(x)
            )
        }

        dim(out) <- dim(x)
    }

    out
}

is_matrix <- function(x) {
    length(dim(x)) == 2 && !is.data.frame(x)
}

class_collapsed <- function(x) {
    paste0(class(x), collapse = "/")
}
