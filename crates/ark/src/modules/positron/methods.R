#
# methods.R
#
# Copyright (C) 2024-2025 Posit Software, PBC. All rights reserved.
#
#

ark_methods_table <- new.env(parent = emptyenv())
ark_methods_table$ark_positron_variable_display_value <- new.env(
    parent = emptyenv()
)
ark_methods_table$ark_positron_variable_display_type <- new.env(
    parent = emptyenv()
)
ark_methods_table$ark_positron_variable_has_children <- new.env(
    parent = emptyenv()
)
ark_methods_table$ark_positron_variable_kind <- new.env(parent = emptyenv())
ark_methods_table$ark_positron_variable_get_child_at <- new.env(
    parent = emptyenv()
)
ark_methods_table$ark_positron_variable_get_children <- new.env(
    parent = emptyenv()
)
ark_methods_table$ark_positron_variable_has_viewer <- new.env(
    parent = emptyenv()
)
ark_methods_table$ark_positron_help_get_handler <- new.env(
    parent = emptyenv()
)
lockEnvironment(ark_methods_table, TRUE)

ark_methods_allowed_packages <- c("torch", "reticulate", "duckplyr")

# check if the calling package is allowed to touch the methods table
check_caller_allowed <- function() {
    if (!in_ark_tests()) {
        # we want the caller of the caller
        calling_env <- .ps.env_name(topenv(parent.frame(2)))
        if (
            !(calling_env %in%
                paste0("namespace:", ark_methods_allowed_packages))
        ) {
            stop(
                "Only allowed packages can (un)register methods. Called from ",
                calling_env
            )
        }
    }
}

check_register_args <- function(generic, class) {
    stopifnot(
        is_string(generic),
        generic %in% names(ark_methods_table),
        typeof(class) == "character"
    )
}

#' Register the methods with the Positron runtime
#'
#' @param generic Generic function name as a character to register
#' @param class Class name as a character
#' @param method A method to be registered. Should be a call object.
#' @export
.ark.register_method <- function(generic, class, method) {
    check_caller_allowed()
    check_register_args(generic, class)

    for (cls in class) {
        assign(cls, method, envir = ark_methods_table[[generic]])
    }
    invisible()
}

#' Unregister a method from the Positron runtime
#'
#' @param generic Generic function name as a character
#' @param class Class name as a character
#' @export
.ark.unregister_method <- function(generic, class) {
    check_caller_allowed()
    check_register_args(generic, class)

    for (cls in class) {
        if (
            exists(cls, envir = ark_methods_table[[generic]], inherits = FALSE)
        ) {
            remove(list = cls, envir = ark_methods_table[[generic]])
        }
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
