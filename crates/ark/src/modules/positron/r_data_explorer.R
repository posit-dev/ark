#
# r_data_explorer.R
#
# Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
#
#

#' @export
.ps.view_data_frame <- function(x, title) {
    # Derive the name of the object from the expression passed to View()
    object_name <- .ps.as_label(substitute(x))

    # Create a title from the name of the object if one is not provided
    if (missing(title)) {
        title <- object_name
    }

    stopifnot(
        is.data.frame(x) || is.matrix(x),
        is.character(title) && length(title) == 1L && !is.na(title)
    )

    # If the variable is defined in the parent frame using the same name as was
    # passed to View(), we can watch it for updates.
    #
    # Note that this means that (for example) View(foo) will watch the variable
    # foo in the parent frame, but Viewing temporary variables like
    # View(cbind(foo, bar)) does not create something that can be watched.
    var <- ""
    env <- NULL
    if (isTRUE(exists(object_name, envir = parent.frame(), inherits = FALSE))) {
        var <- object_name
        env <- parent.frame()
    }

    invisible(.ps.Call("ps_view_data_frame", x, title, var, env))
}

#' @export
.ps.null_count <- function(col) {
    sum(is.na(col))
}

#' @export
.ps.filter_rows <- function(table, row_filters) {
    # Mapping of filter types to parameter arguments
    filter_params <- list(
        compare = "compare_params",
        between = "between_params",
        not_between = "between_params",
        search = "search_params",
        set_membership = "set_membership_params"
    )

    # Create the initial set of indices
    indices <- rep(TRUE, nrow(table))

    for (row_filter in row_filters) {
        # Dynamic dispatch to the appropriate filter function
        filter_function <- paste('.ps.filter_col', row_filter$filter_type, sep = '.')

        # Get the parameters for the filter function. Not all functions have
        # parameters.
        param_name <- filter_params[[row_filter$filter_type]]
        params <- if (is.null(param_name)) {
            NULL
        } else {
            row_filter[[param_name]]
        }

        # Each filter function accepts the column and the parameters as
        # arguments.
        filter_args <- list(table[[row_filter$column_index + 1]], params)

        # Apply the filter function to the column
        if (identical(row_filter$condition, "or")) {
            indices <- indices | do.call(filter_function, filter_args)
        } else {
            indices <- indices & do.call(filter_function, filter_args)
        }
    }

    # Return the indices of the rows that pass all filters
    which(indices)
}

# Filter functions; each accepts a column and a set of parameters

#' @export
.ps.filter_col.compare <- function(col, params) {
    # Form the expression to evaluate. The filter operations map
    # straightforwardly to R's operators, except for the 'equals' operation,
    # which is represented by '=' but needs to be converted to '=='.
    op <- if (identical(params$op, '=')) {
        '=='
    } else {
        params$op
    }

    do.call(op, list(col, params$value))
}

#' @export
.ps.filter_col.not_null <- function(col, params) {
    !is.na(col)
}

#' @export
.ps.filter_col.is_null <- function(col, params) {
    is.na(col)
}

#' @export
.ps.filter_col.between <- function(col, params) {
    col >= params$left_value & col <= params$right_value
}

#' @export
.ps.filter_col.not_between <- function(col, params) {
    !ps.filter_col.between(col, params)
}
