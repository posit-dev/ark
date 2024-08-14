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

.ps.null_count <- function(column, filtered_indices) {
    if (is.null(filtered_indices)) {
        sum(is.na(column))
    } else {
        sum(is.na(column[filtered_indices]))
    }
}

summary_stats_number <- function(col) {

    col <- col[!is.na(col)]

    # Don't compute stats if the column is all NA's or empty.
    if (length(col) == 0) {
        return(numeric(0))
    }

    c(
        min_value = min(col, na.rm = TRUE),
        max_value = max(col, na.rm = TRUE),
        mean = mean(col, na.rm = TRUE),
        median = stats::median(col, na.rm = TRUE),
        stdev = stats::sd(col, na.rm = TRUE)
    )
}

summary_stats_string <- function(col) {
    c(num_empty = sum(!nzchar(col)), num_unique = length(unique(col)))
}

summary_stats_boolean <- function(col) {
    c(true_count = sum(col, na.rm = TRUE), false_count = sum(!col, na.rm = TRUE))
}

summary_stats_date <- function(col) {
    # We have to suppress warnings here because malformed datetimes, eg:
    # x <- as.POSIXct(c("2010-01-01 00:00:00"), tz = "+01:00")
    # When calling `min` on `x` woudl raise a warnings.
    # Turns out, some Parquet files might generate malformed timezones too.
    suppressWarnings({
        list(
            min_date = as.character(min(col, na.rm = TRUE)),
            mean_date = as.character(mean(col, na.rm = TRUE)),
            median_date = as.character(stats::median(col, na.rm = TRUE)),
            max_date = as.character(max(col, na.rm = TRUE)),
            num_unique = length(unique(col))
        )
    })
}

summary_stats_get_timezone <- function(x) {
    # this is the implementation in lubridate for POSIXt objects
    tz <- function (x) {
        tzone <- attr(x, "tzone")
        if (is.null(tzone)) {
            ""
        }
        else {
            tzone[[1]]
        }
    }

    if (!inherits(x, "POSIXt")) {
        stop("Timezone can't be obtained for this object type")
    }

    timezone <- tz(x)

    # When the timezone is reported as "", it will actually be formatted
    # using the system timzeone, so we report it instead.
    if (timezone == "") {
        timezone <- Sys.timezone()
    }

    timezone
}

col_filter_indices <- function(col, idx = NULL) {
    if (!is.null(idx)) {
        col <- col[idx]
    }
    col
}

.ps.filter_rows <- function(table, row_filters) {
    # Are we working with a matrix here?
    is_matrix <- is.matrix(table)

    # Create the initial set of indices
    indices <- rep(TRUE, nrow(table))
    row_filters_errors <- character(length(row_filters))

    for (i in seq_along(row_filters)) {
        row_filter <- row_filters[[i]]

        # Do not try to apply filters that are already marked as invalid.
        if (!is.null(row_filter$is_valid) && !row_filter$is_valid) {
            row_filters_errors[i] <- row_filter$error_message %||% "Invalid filter for unknown reason"
            next
        }

        # Dynamic dispatch to the appropriate filter function
        filter_function <- paste('.ps.filter_col', row_filter$filter_type, sep = '.')

        # Get the parameters for the filter function. Not all functions have
        # parameters.
        params <- row_filter$params

        # Each filter function accepts the column and the parameters as
        # arguments.
        col <- if (is_matrix) {
            table[, row_filter$column_schema$column_index + 1, drop = TRUE]
        } else {
            table[[row_filter$column_schema$column_index + 1]]
        }
        filter_args <- list(col, params)

        row_filters_errors[i] <- tryCatch({
            # Apply the filter function to the column
            filter_matches <- do.call(filter_function, filter_args)
            if (identical(row_filter$condition, "or")) {
                indices <- indices | filter_matches
            } else {
                indices <- indices & filter_matches
            }
            NA
        }, error = function(e) {
            e$message
        })
    }

    # Return the indices of the rows that pass all filters
    list(
        indices = which(indices),
        errors = row_filters_errors
    )
}

# Filter functions; each accepts a column and a set of parameters

.ps.filter_col.compare <- function(col, params) {
    # Form the expression to evaluate. The filter operations map
    # straightforwardly to R's operators, except for the 'equals' operation,
    # which is represented by '=' but needs to be converted to '=='.
    op <- switch(params$op,
        `=` = "==",
        `!=` = "!=",
        `>` = ">",
        `>=` = ">=",
        `<` = "<",
        `<=` = "<=",
        stop("Unsupported comparison operator '", params$op, "'")
    )

    # Values are always marshaled as strings at the RPC layer, so coerce them to
    # numeric if the column is numeric.
    value <- if (is.numeric(col)) {
        as.numeric(params$value)
    } else {
        params$value
    }

    do.call(op, list(col, value))
}

.ps.filter_col.not_null <- function(col, params) {
    !is.na(col)
}

.ps.filter_col.is_null <- function(col, params) {
    is.na(col)
}

.ps.filter_col.is_empty <- function(col, params) {
    !nzchar(col)
}

.ps.filter_col.not_empty <- function(col, params) {
    nzchar(col)
}

.ps.filter_col.is_true <- function(col, params) {
    col & !is.na(col)
}

.ps.filter_col.is_false <- function(col, params) {
    !col & !is.na(col)
}

.ps.filter_col.between <- function(col, params) {
    # Coerce values to numeric if the column is numeric
    is_numeric <- is.numeric(col)
    left_value <- if (is_numeric) {
        as.numeric(params$left_value)
    } else {
        params$left_value
    }
    right_value <- if (is_numeric) {
        as.numeric(params$right_value)
    } else {
        params$right_value
    }

    # Look for values between the left and right values
    col >= left_value & col <= right_value
}

.ps.filter_col.not_between <- function(col, params) {
    !.ps.filter_col.between(col, params)
}

.ps.regex_escape <- function(x) {
    # Escape all regex magic characters in a string
    gsub("([][{}()+*^$|\\\\?.])", "\\\\\\1", x)
}

.ps.filter_col.search <- function(col, params) {
    # Search for the term anywhere in the column's values
    if (identical(params$search_type, "contains")) {
        # We escape the term to ensure that it is treated as a fixed string; we
        # can't do this using `fixed = TRUE` since `ignore.case` only works when
        # `fixed = FALSE`
        escaped_term <- .ps.regex_escape(params$term)
        grepl(pattern = escaped_term, col, fixed = FALSE, ignore.case = !params$case_sensitive)
    }

    # Search for the term at the beginning of the column's values
    else if (identical(params$search_type, "starts_with")) {
        escaped_term <- .ps.regex_escape(params$term)
        grepl(pattern = paste0("^", escaped_term), col, ignore.case = !params$case_sensitive)
    }

    # Search for the term at the end of the column's values
    else if (identical(params$search_type, "ends_with")) {
        escaped_term <- .ps.regex_escape(params$term)
        grepl(pattern = paste0(escaped_term, "$"), col, ignore.case = !params$case_sensitive)
    }

    # Search for the term anywhere in the column's values, as a regular
    # expression
    else if (identical(params$search_type, "regex_match")) {
        grepl(pattern = params$term, col, ignore.case = !params$case_sensitive)
    }

    # Unsupported search type
    else {
        stop("Unsupported search type '", params$search_type, "'")
    }
}

.ps.table_subset <- function(x, i, j) {
    if (inherits(x, "data.frame")) {
        # drop additional classes, so data we dont dispatch to subclasses methods
        # like `[.tibble` or `[.data.table`.
        class(x) <- "data.frame"
    }
    x[i, j, drop = FALSE]
}

is_na_checked <- function(x) {
    result <- is.na(x)
    stopifnot(is.logical(result), length(x) == length(result))
    result
}

export_selection <- function(x, format = c("csv", "tsv", "html"), include_header = TRUE) {
    format <- match.arg(format)

    if (format == "csv") {
        write_delim(x, delim = ",", include_header)
    } else if (format == "tsv") {
        write_delim(x, delim = "\t", include_header)
    } else if (format == "html") {
        write_html(x, include_header)
    } else {
        stop("Unsupported format: ", format)
    }
}

write_delim <- function(x, delim, include_header) {
    tmp <- tempfile()
    defer(unlink(tmp))

    utils::write.table(x, tmp, sep = delim, row.names = FALSE, col.names = include_header, quote = FALSE, na = "")
    # We use size - 1 because we don't want to read the last newline character
    # that creates problems when pasting the content in spreadsheets
    readChar(tmp, file.info(tmp)$size - 1L)
}

write_html <- function(x, include_header) {
    # TODO: do not depend on knitr to render html tables
    # kable takes NA to mean "use the default column names"
    # and `NULL` means no column names
    col_names <- if(include_header) {
        NA
    } else {
        NULL
    }
    local_options(knitr.kable.NA = "") # use empty strings for NA's
    knitr::kable(x, format = "html", row.names = FALSE, col.names = col_names)
}
