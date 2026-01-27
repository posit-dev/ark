#
# methods.R
#
# Copyright (C) 2024-2025 Posit Software, PBC. All rights reserved.
#
#

ark_methods_table <- new.env(parent = emptyenv())

#' Customize display value for objects in Variables Pane
#'
#' @param x Object to get the display value for
#' @param ... Additional arguments (unused)
#' @param width Maximum expected width. This is just a suggestion, the UI
#'   can still truncate the string to different widths.
#' @return A length 1 character vector containing the display value
ark_methods_table$ark_positron_variable_display_value <- new.env(
    parent = emptyenv()
)

#' Customize display type for objects in Variables Pane
#'
#' @param x Object to get the display type for
#' @param ... Additional arguments (unused)
#' @param include_length Boolean indicating whether to include object length
#' @return A length 1 character vector describing the object type
ark_methods_table$ark_positron_variable_display_type <- new.env(
    parent = emptyenv()
)

#' Check if object has inspectable children in Variables Pane
#'
#' @param x Object to check for children
#' @param ... Additional arguments (unused)
#' @return Logical value: TRUE if the object can be inspected, FALSE otherwise
ark_methods_table$ark_positron_variable_has_children <- new.env(
    parent = emptyenv()
)

#' Specify variable kind for Variables Pane organization
#'
#' @param x Object to get the variable kind for
#' @param ... Additional arguments (unused)
#' @return Length 1 character vector specifying the kind of variable (e.g., "table", "other")
#' See the `pub enum VariableKind` for all accepted types.
ark_methods_table$ark_positron_variable_kind <- new.env(parent = emptyenv())

#' Get specific child element from object for Variables Pane inspection
#'
#' @param x Object to get child from
#' @param ... Additional arguments (unused)
#' @param index Integer > 1, representing the index position of the child
#' @param name Character string or NULL, the name of the child
#' @return The child object at the specified index/name
ark_methods_table$ark_positron_variable_get_child_at <- new.env(
    parent = emptyenv()
)

#' Control viewer availability for objects in Variables Pane
#'
#' @param x Object to check for viewer support
#' @param ... Additional arguments (unused)
#' @return Logical value: TRUE if viewer should be enabled, FALSE to disable
ark_methods_table$ark_positron_variable_has_viewer <- new.env(
    parent = emptyenv()
)

#' Get child objects for Variables Pane inspection
#'
#' @param x Object to get children from
#' @param ... Additional arguments (unused)
#' @return Named list of child objects to be displayed.
#' The above methods are called in the elements of this list to make the display
#' of child objects consistent.
ark_methods_table$ark_positron_variable_get_children <- new.env(
    parent = emptyenv()
)

#' Get the help handler for an R object
#'
#' @param obj An R object to obtain the help handler for.
#'
#' @returns Returns a help handler or `NULL` if
#' the object can't be handled.
#'
#' The returned help handler is a function with no arguments that is expected to
#' show the help documentation for the object as a side effect and return
#' `TRUE` if it was able to do so, or `FALSE` otherwise.
#'
#' It may use e.g `.ps.help.browse_external_url` to display a URL
#' in the help pane.
ark_methods_table$ark_positron_help_get_handler <- new.env(
    parent = emptyenv()
)

#' Custom view action for objects in Variables Pane
#'
#' @param x Object to view
#' @param ... Additional arguments (unused)
#' @return Logical value: TRUE on success, FALSE otherwise
ark_methods_table$ark_positron_variable_view <- new.env(
    parent = emptyenv()
)

lockEnvironment(ark_methods_table, TRUE)

ark_methods_allowed_packages <- c(
    "torch",
    "reticulate",
    "duckplyr",
    "connections"
)

# check if the calling package is allowed to touch the methods table
check_caller_allowed <- function() {
    if (!in_ark_tests()) {
        # we want the caller of the caller
        calling_env <- .ps.env_name(topenv(parent.frame(2)))

        if (calling_env == "package:base") {
            # allow base for internal calls
            return()
        }

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

    # Get all classes to check, including S4 superclasses
    classes <- class(object)
    if (isS4(object)) {
        # For S4 objects, get the full inheritance hierarchy
        classes <- methods::extends(class(object))
    }

    for (cls in classes) {
        if (!is.null(method <- get0(cls, envir = methods_table))) {
            return(eval(
                as.call(list(method, object, ...)),
                envir = globalenv()
            ))
        }
    }

    NULL
}
