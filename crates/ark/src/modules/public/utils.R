#
# utils.R
#
# Copyright (C) 2022 Posit Software, PBC. All rights reserved.
#
#

.ps.Call <- function(.NAME, ...) {
    .Call(.NAME, ..., PACKAGE = "(embedding)")
}

.ps.inspect <- function(item) {
    .Internal(inspect(item))
}

.ps.objectId <- function(object) {
    .ps.Call("ps_object_id", object)
}

.ps.recursiveSearch <- function(object, callback, ...) {

    result <- callback(object, ...)
    if (!is.null(result))
        return(result)

    if (is.recursive(object)) {
        for (i in seq_along(object)) {
            result <- .ps.recursiveSearch(object[[i]], callback, ...)
            if (!is.null(result))
                return(result)
        }
    }

}

.ps.ark.version <- function() {
    # Read the version information from Ark
    ark_version <- .ps.Call("ps_ark_version")

    # Format the date into the current timezone for display
    if (nzchar(ark_version['date'])) {
        utc_date <- as.POSIXct(ark_version['date'],
                               format = "%Y-%m-%dT%H:%M:%SZ",
                               tz = "UTC")
        local_date <- format(utc_date,
                             format = "%Y-%m-%d %H:%M:%S",
                             usetz = TRUE,
                             tz = Sys.timezone())
        ark_version['date'] <- local_date
    }

    ark_version
}

# Sleep that doesn't check for interrupts to test an unresponsive runtime.
.ps.deep_sleep <- function(secs) {
    .ps.Call("ps_deep_sleep", secs)
}

# Extracts a character label from a syntactically valid quoted R expression
.ps.as_label <- function(expr) {
    paste(deparse(expr, backtick = TRUE), collapse = "")
}

# Converts an R object to JSON (returned as a string)
.ps.to_json <- function(object) {
    .ps.Call("ps_to_json", object)
}
