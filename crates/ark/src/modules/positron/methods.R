#
# methods.R
#
# Copyright (C) 2024 Posit Software, PBC. All rights reserved.
#
#

ark_methods_table <- new.env(parent = emptyenv())
ark_methods_table$ark_positron_variable_display_value <- new.env(parent = emptyenv())
ark_methods_table$ark_positron_variable_display_type <- new.env(parent = emptyenv())
ark_methods_table$ark_positron_variable_has_children <- new.env(parent = emptyenv())
ark_methods_table$ark_positron_variable_kind <- new.env(parent = emptyenv())
lockEnvironment(ark_methods_table, TRUE)

#' Register the methods with the Positron runtime
#'
#' @param generic Generic function name as a character to register
#' @param class Class name as a character
#' @param method A method to be registered. Should be a call object.
#' @export
.ark.register_ark_method <- function(generic, class, method) {
    stopifnot(
        is_string(generic),
        generic %in% names(ark_methods_table),
        typeof(class) == "character"
    )
    for (cls in class) {
        assign(cls, method, envir = ark_methods_table[[generic]])
    }
    invisible()
}

call_ark_method <- function(generic, object, ...) {
    methods_table <- ark_methods_table[[generic]]

    if (is.null(methods_table)) {
        return(NULL)
    }

    for (cls in class(object)) {
        if (!is.null(method <- get0(cls, envir = methods_table))) {
            return(eval(
                as.call(list(method, object, ...)),
                envir = globalenv()
            ))
        }
    }

    NULL
}
