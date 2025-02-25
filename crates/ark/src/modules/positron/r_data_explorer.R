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

.ps.null_count <- function(column) {
    sum(is.na(column))
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
    if (inherits(col, 'haven_labelled')) {
        col <- haven::as_factor(col)
    }

    if (is.factor(col)) {
        # We could have an optimization here to get unique and empty values
        # from levels, but probably not worth it.
        col <- as.character(col)
    }

    c(num_empty = sum(!nzchar(col)), num_unique = length(unique(col)))
}

summary_stats_boolean <- function(col) {
    c(
        true_count = sum(col, na.rm = TRUE),
        false_count = sum(!col, na.rm = TRUE)
    )
}

summary_stats_date <- function(col) {
    # We have to suppress warnings here because malformed datetimes, eg:
    # x <- as.POSIXct(c("2010-01-01 00:00:00"), tz = "+01:00")
    # When calling `min` on `x` would raise a warning.
    # Turns out, some Parquet files might generate malformed timezones too.
    suppressWarnings({
        # When all values in the column are NA's, there min and max return -Inf and +Inf,
        # mean returns NaN and median returns NA. We make everything return `NULL` so we
        # correctly display the values in the front-end.
        min_date <- finite_or_null(min(col, na.rm = TRUE))
        max_date <- finite_or_null(max(col, na.rm = TRUE))
        mean_date <- finite_or_null(mean(col, na.rm = TRUE))
        median_date <- finite_or_null(stats::median(col, na.rm = TRUE))
        list(
            min_date = as.character(min_date),
            mean_date = as.character(mean_date),
            median_date = as.character(median_date),
            max_date = as.character(max_date),
            num_unique = length(unique(col))
        )
    })
}

finite_or_null <- function(x) {
    if (!is.finite(x)) {
        NULL
    } else {
        x
    }
}

summary_stats_get_timezone <- function(x) {
    # this is the implementation in lubridate for POSIXt objects
    tz <- function(x) {
        tzone <- attr(x, "tzone")
        if (is.null(tzone)) {
            ""
        } else {
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
            row_filters_errors[i] <- row_filter$error_message %||%
                "Invalid filter for unknown reason"
            next
        }

        # Dynamic dispatch to the appropriate filter function
        filter_function <- paste(
            '.ps.filter_col',
            row_filter$filter_type,
            sep = '.'
        )

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

        row_filters_errors[i] <- tryCatch(
            {
                # Apply the filter function to the column
                filter_matches <- do.call(filter_function, filter_args)
                if (identical(row_filter$condition, "or")) {
                    indices <- indices | filter_matches
                } else {
                    indices <- indices & filter_matches
                }
                NA
            },
            error = function(e) {
                e$message
            }
        )
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
    op <- switch(
        params$op,
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
    if (identical(params$search_type, "contains")) {
        # Search for the term anywhere in the column's values

        # We escape the term to ensure that it is treated as a fixed string; we
        # can't do this using `fixed = TRUE` since `ignore.case` only works when
        # `fixed = FALSE`
        escaped_term <- .ps.regex_escape(params$term)
        grepl(
            pattern = escaped_term,
            col,
            fixed = FALSE,
            ignore.case = !params$case_sensitive
        )
    } else if (identical(params$search_type, "not_contains")) {
        # Search for the term not contained in the column's values
        escaped_term <- .ps.regex_escape(params$term)
        !grepl(
            pattern = escaped_term,
            col,
            fixed = FALSE,
            ignore.case = !params$case_sensitive
        )
    } else if (identical(params$search_type, "starts_with")) {
        # Search for the term at the beginning of the column's values
        escaped_term <- .ps.regex_escape(params$term)
        grepl(
            pattern = paste0("^", escaped_term),
            col,
            ignore.case = !params$case_sensitive
        )
    } else if (identical(params$search_type, "ends_with")) {
        # Search for the term at the end of the column's values
        escaped_term <- .ps.regex_escape(params$term)
        grepl(
            pattern = paste0(escaped_term, "$"),
            col,
            ignore.case = !params$case_sensitive
        )
    } else if (identical(params$search_type, "regex_match")) {
        # Search for the term anywhere in the column's values, as a regular
        # expression
        # We suppress warnings because invalid regex will raise both an error and a warning.
        # We already catch the error when calling this functions, but the warning leaks to
        # the user console.
        suppressWarnings(grepl(
            pattern = params$term,
            col,
            ignore.case = !params$case_sensitive
        ))
    } else {
        # Unsupported search type
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

export_selection <- function(
    x,
    format = c("csv", "tsv", "html"),
    include_header = TRUE
) {
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
    path <- tempfile()
    defer(unlink(path))

    write_delim_impl(x, path, delim, include_header)

    # We use `size - 1` because we don't want to read the last newline character
    # as that creates problems when pasting the content in spreadsheets.
    # `file.info()$size` reports the size in bytes, hence `useBytes = TRUE`.
    readChar(path, file.info(path)$size - 1L, useBytes = TRUE)
}

write_delim_impl <- function(x, path, delim, include_header) {
    # Scope the `con` lifetime to just this helper.
    # We need to `close()` the connection before we try and get the
    # `file.info()$size`.

    # Must open in binary write mode, otherwise even though we set
    # `eol = "\n"` on Windows it will still write `\r\n` due to the C
    # level `vfprintf()` call.
    con <- file(path, open = "wb")
    defer(close(con))

    utils::write.table(
        x = x,
        file = con,
        sep = delim,
        eol = "\n",
        row.names = FALSE,
        col.names = include_header,
        quote = FALSE,
        na = ""
    )
}

write_html <- function(x, include_header) {
    # TODO: do not depend on knitr to render html tables
    # kable takes NA to mean "use the default column names"
    # and `NULL` means no column names
    col_names <- if (include_header) {
        NA
    } else {
        NULL
    }
    local_options(knitr.kable.NA = "") # use empty strings for NA's
    knitr::kable(x, format = "html", row.names = FALSE, col.names = col_names)
}

profile_histogram <- function(
    x,
    method = c("fixed", "sturges", "fd", "scott"),
    num_bins = NULL,
    quantiles = NULL
) {
    # We only use finite values for building this histogram.
    # This removes NA's, Inf, NaN and -Inf
    x <- x[is.finite(x)]

    # No-non finite values, we early return as there's nothing we can compute.
    if (length(x) == 0) {
        return(list(
            bin_edges = c(),
            bin_counts = c(),
            quantiles = rep(NA_real_, length(quantiles))
        ))
    }

    # If we have a Date variable, we convert it to POSIXct, in order to be able to have equal
    # width bins - that can be fractions of dates.
    if (inherits(x, "Date")) {
        x <- as.POSIXct(x)
    }

    if (!is.null(quantiles)) {
        quantiles <- stats::quantile(x, probs = quantiles)
    } else {
        # We otherwise return an empty quantiles vector
        quantiles <- NULL
    }

    method <- match.arg(method)
    num_bins <- histogram_num_bins(x, method, num_bins)

    # For dates, `hist` does not compute the number of breaks automatically
    # using default methods.
    # We force something considering the integer representation.
    if (inherits(x, "POSIXct") || is.integer(x)) {
        # The pretty bins algorithm doesn't really make sense for dates,
        # so we generate our own bin_edges for them.
        breaks <- seq(min(x), max(x), length.out = num_bins + 1L)
    } else {
        breaks <- num_bins
        stopifnot(is.integer(breaks), length(breaks) == 1)
    }

    # A warning is raised when computing the histogram for dates due to
    # integer overflow in when computing n cell midpoints, but we don't
    # use it, so it can be safely ignored.
    suppressWarnings(h <- graphics::hist(x, breaks = breaks, plot = FALSE))

    bin_edges <- h$breaks
    bin_counts <- h$counts

    # For dates, we convert back the breaks to the date representation.
    if (inherits(x, "POSIXct")) {
        # Must supply an `origin` on R <= 4.2
        origin <- as.POSIXct("1970-01-01", tz = "UTC")
        bin_edges <- as.POSIXct(
            h$breaks,
            tz = attr(x, "tzone"),
            origin = origin
        )
    }

    list(
        bin_edges = bin_edges,
        bin_counts = bin_counts,
        quantiles = quantiles
    )
}

profile_frequency_table <- function(x, limit) {
    x <- x[!is.na(x)]

    if (length(x) == 0) {
        return(list(
            values = NULL,
            counts = NULL,
            other_count = 0
        ))
    }

    if (inherits(x, "haven_labelled")) {
        x <- haven::as_factor(x)
    }

    if (is.factor(x)) {
        values <- levels(x)
        counts <- table(x)
    } else {
        # We don't use `table` directly because we don't want to loose the type
        # of value types so they can be formatted with our formatting routines.
        values <- unique(x)
        counts <- tabulate(match(x, values))
    }

    index <- utils::head(order(counts, decreasing = TRUE), limit)
    values <- values[index]
    counts <- counts[index]
    other_count <- length(x) - sum(counts)

    list(
        values = values,
        counts = counts,
        other_count = other_count
    )
}

histogram_num_bins <- function(x, method, fixed_num_bins) {
    num_bins <- if (method == "sturges") {
        grDevices::nclass.Sturges(x)
    } else if (method == "fd") {
        # FD calls into signif, which is not implemented for Dates
        grDevices::nclass.FD(unclass(x))
    } else if (method == "scott") {
        grDevices::nclass.scott(x)
    } else if (method == "fixed") {
        fixed_num_bins
    } else {
        stop("Unknow method :", method)
    }

    if (is.integer(x) || inherits(x, "POSIXct")) {
        # For integers, we don't want num_bins to be larger than the width of the range
        # so we replace it if necessary.
        min_value <- min(x)
        max_value <- max(x)
        width <- max_value - min_value

        if (as.integer(width) < num_bins) {
            num_bins <- as.integer(width) + 1L
        }
    }

    # Honor the maximum amount of bins requested by the front-end.
    if (!is.null(fixed_num_bins) && num_bins > fixed_num_bins) {
        num_bins <- fixed_num_bins
    }

    as.integer(num_bins)
}
