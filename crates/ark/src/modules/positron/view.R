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
        top_level <- sys.nframe() == 1
        return(view_function(x, title, var, env, top_level = top_level))
    }

    stop(sprintf(
        "Can't `View()` an object of class `%s`",
        paste(class(x), collapse = "/")
    ))
}

view_function <- function(x, title, var = "", env = NULL, top_level = FALSE) {
    stopifnot(is.function(x))

    # Only resource the namespace if we're at top-level. Doing it while
    # arbitrary code is running is unsafe as the source references are mutated
    # globally. The mutation could invalidate assumptions made by running code.
    if (top_level) {
        fn_populate_srcref(x)
    }

    # Get srcref _after_ potentially resourcing from a virtual namespace file
    info <- srcref_info(attr(x, "srcref"))
    if (
        !is.null(info) &&
            !is.null(info$file) &&
            (is_ark_uri(info$file) || file.exists(info$file))
    ) {
        # Request frontend to display file
        .ps.ui.navigateToFile(
            info$file,
            info$range$start_line,
            info$range$start_column
        )
        return(invisible())
    }

    # We don't have a valid source reference to point to so we'll create a new
    # virtual document and open that instead
    if (!is.null(info$lines)) {
        # The srcref might not point to a valid file but might contain a full
        # source. That's the case when calling `parse()` manually. This source
        # is more accurate than deparsing so we use that.
        contents <- paste_line(info$lines)
    } else {
        # Deparse as fallback
        contents <- paste_line(deparse(x))
    }

    if (is.null(env)) {
        env_name <- "unknown"
    } else {
        env_name <- .ps.env_name(env) %||% obj_address(env)
    }

    if (!nzchar(var)) {
        var <- "unknown"
    }

    # NOTE: We currently open a new virtual document that never gets cleaned up.
    # Getting notified of editor close by the frontend would be complex to set
    # up correctly. Instead this could be fixed by sending the document to the
    # frontend and let it manage the lifetime of the virtual docs.
    uri <- ark_uri(sprintf("%s/%s.R", env_name, var))
    insert_virtual_document(uri, contents)

    .ps.ui.navigateToFile(uri)
    invisible()
}

insert_virtual_document <- function(uri, contents) {
    .ps.Call("ps_insert_virtual_document", uri, contents)
}

ark_uri <- function(path) {
    .ps.Call("ps_ark_uri", path)
}

is_ark_uri <- function(path) {
    startsWith(path, "ark:")
}
