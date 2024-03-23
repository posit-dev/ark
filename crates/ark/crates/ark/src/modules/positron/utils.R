#
# utils.R
#
# Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
#
#

#' @export
.ps.inspect <- function(item) {
    .Internal(inspect(item))
}

#' @export
.ps.objectId <- function(object) {
    .ps.Call("ps_object_id", object)
}

#' @export
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

#' @export
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
#' @export
.ps.deep_sleep <- function(secs) {
    .ps.Call("ps_deep_sleep", secs)
}

# Extracts a character label from a syntactically valid quoted R expression
#' @export
.ps.as_label <- function(expr) {
    paste(deparse(expr, backtick = TRUE), collapse = "")
}

# Converts an R object to JSON (returned as a string)
#' @export
.ps.to_json <- function(object) {
    .ps.Call("ps_to_json", object)
}

# Evaluate expression in positron's namespace (which includes access to the
# private modules). Any features accessible from `.ps.internal()` are
# subject to change without notice.
#' @export
.ps.internal <- function(expr) {
    expr <- substitute(expr)

    # Retrieve function from internal namespace
    expr[[1]] <- eval(expr[[1]], parent.env(environment()))

    # Evaluate arguments in calling frame
    eval(expr, parent.frame())
}

# From `rlang::env_name()`
#' @export
.ps.env_name <- function(env) {
    if (typeof(env) != "environment") {
        return(NULL)
    }

    if (identical(env, globalenv())) {
        return("global")
    }
    if (identical(env, baseenv())) {
        return("package:base")
    }
    if (identical(env, emptyenv())) {
        return("empty")
    }

    nm <- environmentName(env)

    if (isNamespace(env)) {
        return(paste0("namespace:", nm))
    }

    if (nzchar(nm)) {
        nm
    } else {
        NULL
    }
}
