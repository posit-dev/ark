#
# s3.R
#
# Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
#
#

#' @export
.ps.s3.genericNameCache <- new.env(parent = emptyenv())

#' @export
.ps.s3.genericNameFromFunction <- function(callable) {
    # Check whether we can safely cache the result.
    package <- fn_package_name(callable)
    if (is.null(package)) {
        # Can't cache
        return(.ps.s3.genericNameFromFunctionImpl(callable))
    }

    id <- .ps.objectId(callable)
    .ps.s3.genericNameCache[[id]] <-
        .ps.s3.genericNameCache[[id]] %??%
        .ps.s3.genericNameFromFunctionImpl(callable)
}

#' @export
.ps.s3.genericNameFromFunctionImpl <- function(callable) {
    useMethodSym <- as.name("UseMethod")
    value <- .ps.recursiveSearch(body(callable), function(node) {
        if (
            is.call(node) &&
                length(node) >= 2L &&
                identical(node[[1L]], useMethodSym) &&
                is.character(node[[2L]])
        ) {
            return(node[[2L]])
        }
    })

    as.character(value)
}
