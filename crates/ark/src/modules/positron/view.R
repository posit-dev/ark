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

    if (is.function(x)) {
        return(view_function(x, title, var, env))
    }

    stop(sprintf(
        "Can't `View()` an object of class `%s`",
        paste(class(x), collapse = "/")
    ))
}

view_function <- function(x, title, var, env) {
    stopifnot(is.function(x))

    info <- srcref_info(attr(x, "srcref"))
    if (!is.null(info)) {
        if (is.null(info$file)) {
            contents <- paste_line(info$lines)
            .ps.ui.newDocument(contents, "r")
            return(invisible())
        }

        if (file.exists(info$file)) {
            # Request frontend to display file
            .ps.ui.navigateToFile(
                info$file,
                info$range$start_line,
                info$range$start_column
            )
            return(invisible())
        }

        # fallthrough
    }

    # TODO: Currently this opens the file in an untitled editor. This is not
    # ideal as the user will be asked to save the file on close. In the future,
    # the contents should be sent to positron-r as a document to open via a
    # TextContent provider to give the editor a "virtual document" flair.
    #
    # Note that we don't provide the document from the backend side because that
    # would require us to manage its lifetime in some way. Better do all that on
    # the backend side that introduce more communication about editor lifetimes.
    contents <- paste_line(deparse(x))
    .ps.ui.newDocument(contents, "r")

    return(invisible())
}

paste_line <- function(x) {
    paste(x, collapse = "\n")
}
