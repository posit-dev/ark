#
# methods.R
#
# Copyright (C) 2024 Posit Software, PBC. All rights reserved.
#
#

ark_methods_table <- new.env(parent = emptyenv())

register_ark_method <- function(generic, class, method) {
    method <- substitute(method)
    if (!exists(generic, envir = ark_methods_table)) {
        ark_methods_table[[generic]] <- new.env(parent = emptyenv())
    }
    ark_methods_table[[generic]][[class]] <- method
}

call_ark_method <- function(generic, object, ...) {
    methods_table <- ark_methods_table[[generic]]

    if (is.null(methods_table)) {
        stop("No methods registered for generic '", generic, "'")
    }

    for (cl in class(object)) {
        if (exists(cl, envir = methods_table)) {
            return(do.call(eval(methods_table[[cl]], envir = globalenv()), list(object, ...)))
        }
    }

    stop("No methods found for object")
}

#' Register the methods with the Positron runtime
#'
#' @param generic Generic function name as a character to register
#' @param class class name as a character
#' @param method A method to be registered. Should be a call object.
.ps.register_ark_method <- function(generic, class, method) {
    # Even though methods are stored in an R environment, we call into Rust
    # to make sure we have a single source of truth for generic names.

    # Functions are inlined, so we always construct the call into them.
    # this also allows registering with eg: `pkg::fun` without explicitly
    # using the function from that namespace, which might change at points
    # in time.
    if (is.function(method)) {
        method <- substitute(method)
    }

    .ps.Call("ps_register_ark_method", generic, class, substitute(method))
}
