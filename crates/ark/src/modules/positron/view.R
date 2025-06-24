#
# view.R
#
# Copyright (C) 2023-2025 Posit Software, PBC. All rights reserved.
#
#

# Dispatches to object handlers. The handlers take `var` and `env` arguments.
# These are only passed if we could infer a variable name from the input and if
# that variable exists in the calling environment. This is used for live
# updating the objects, if supported (e.g. data frames in the data viewer).
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

view_function <- function(
    x,
    title = "",
    var = "",
    env = NULL,
    top_level = FALSE
) {
    stopifnot(is.function(x))

    info <- view_function_info(
        x,
        title,
        var = var,
        env = env,
        top_level = top_level
    )

    switch(
        info$kind,

        vdoc = {
            insert_virtual_document(info$uri, info$contents)
        },

        srcref = {
            # Only non-NULL if a new vdoc for a namespace was generated
            if (!is.null(info$contents)) {
                insert_virtual_document(info$uri, info$contents)
            }
        }
    )

    .ps.ui.navigateToFile(
        info$uri,
        line = info$line,
        column = info$column
    )

    invisible()
}

view_function_info <- function(
    x,
    title = "",
    var = "",
    env = NULL,
    top_level = FALSE
) {
    stopifnot(is.function(x))

    # Only resource the namespace if we're at top-level. Doing it while
    # arbitrary code is running is unsafe as the source references are mutated
    # globally. The mutation could invalidate assumptions made by running code.
    if (top_level) {
        # `NULL` if srcref are already present or couldn't be generated
        ns_srcref_info <- fn_populate_srcref_without_vdoc_insertion(x)

        # Extract contents, if any from `list(uri, contents)`. Ideally would be
        # a named list but currently inconvenient to do across FFI boundary.
        ns_srcref_info <- ns_srcref_info[[2]]
    } else {
        ns_srcref_info <- NULL
    }

    # Get srcref _after_ potentially resourcing from a virtual namespace file
    info <- srcref_info(attr(x, "srcref"))
    if (
        !is.null(info) &&
            !is.null(info$file) &&
            (is_ark_uri(info$file) || file.exists(info$file))
    ) {
        return(list(
            kind = "srcref",
            uri = info$file,
            contents = ns_srcref_info,
            line = info$range$start_line,
            column = info$range$start_column
        ))
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

    env_name <- .ps.env_name(env) %||% obj_address(env)

    if (!nzchar(var)) {
        var <- "unknown"
    }

    # NOTE: We currently open a new virtual document that never gets cleaned up.
    # Getting notified of editor close by the frontend would be complex to set
    # up correctly. Instead this could be fixed by sending the document to the
    # frontend and let it manage the lifetime of the virtual docs.
    uri <- ark_uri(sprintf("%s/%s.R", env_name, var))

    list(
        kind = "vdoc",
        uri = uri,
        contents = contents,
        line = 0L,
        column = 0L
    )
}

# For unit tests
view_function_test <- function(x, var, env) {
    info <- view_function_info(
        x,
        var = var,
        env = env,
        top_level = TRUE
    )

    paste_line(c(
        sprintf("URI: %s", info$uri),
        "",
        # Expected to be `NULL` for srcref case
        info$contents
    ))
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

ark_ns_uri <- function(path) {
    .ps.Call("ps_ark_ns_uri", path)
}
