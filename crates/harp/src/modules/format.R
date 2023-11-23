#
# format.R
#
# Copyright (C) 2023 Posit Software, PBC. All rights reserved.
#
#

# Works around possibly unconforming methods in `base::format()`. Tries
# hard to recover from failed assumptions, including by unclassing and
# reformatting with the default method.
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
        log_warning(sprintf(
            "`format()` method for <%s> should return a character vector.",
            class_collapsed(x)
        ))
        return(format_fallback(x, ...))
    }

    # Try to recover if dimensions don't agree (for example `format.Surv()`
    # doesn't preserve dimensions, see https://github.com/posit-dev/positron/issues/1862)
    if (!identical(dim(x), dim(out))) {
        log_warning(sprintf(
            "`format()` method for <%s> should return conforming dimensions.",
            class_collapsed(x)
        ))

        if (length(x) != length(out)) {
            log_warning(sprintf(
                "`format()` method for <%s> should return the same number of elements.",
                class_collapsed(x)
            ))
            return(format_fallback(x, ...))
        }

        dim(out) <- dim(x)
    }

    out
}

# Try without dispatch
format_fallback <- function(x, ...) {
    out <- base::format(unclass(x), ...)

    # Shouldn't happen but just in case
    if (!is.character(out)) {
        stop("Unexpected type from `base::format()`.")
    }
    if (length(x) != length(out)) {
        stop("Unexpected length from `base::format()`.")
    }
    if (!identical(dim(x), dim(out))) {
        stop("Unexpected dimensions from `base::format()`.")
    }

    out
}
