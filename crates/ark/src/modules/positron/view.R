#
# view.R
#
# Copyright (C) 2023-2025 Posit Software, PBC. All rights reserved.
#
#

view <- function(x, title) {
    # Derive the name of the object from the expression passed to View()
    name <- as_label(substitute(x))

    # Create a title from the name of the object if one is not provided
    if (missing(title)) {
        title <- name
    }
    stopifnot(is_string(title))

    # If the variable is defined in the parent frame using the same name as was
    # passed to View(), we can watch it for updates.
    #
    # Note that this means that (for example) View(foo) will watch the variable
    # foo in the parent frame, but Viewing temporary variables like
    # View(cbind(foo, bar)) does not create something that can be watched.
    if (isTRUE(exists(name, envir = parent.frame(), inherits = FALSE))) {
        var <- name
        env <- parent.frame()
    } else {
        var <- ""
        env <- NULL
    }

    if (is_viewable_data_frame(x)) {
        return(view_data_frame(x, title, var, env))
    }

    stop(sprintf(
        "Can't `View()` an object of class `%s`",
        paste(class(x), collapse = "/")
    ))
}
