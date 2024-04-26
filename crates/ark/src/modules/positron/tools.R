#
# tools.R
#
# Copyright (C) 2023 Posit Software, PBC. All rights reserved.
#
#

`%||%` <- function(x, y) {
    if (length(x) || is.environment(x)) x else y
}

`%??%` <- function(x, y) {
    if (is.null(x)) y else x
}

ensure_directory <- function(path) {
    exists <- dir.exists(path)

    if (all(exists)) {
        # Nothing to do if they all exist
        return(invisible())
    }

    path <- path[!exists]

    # Try to create missing ones (`dir.create()` isn't vectorized)
    for (elt in path) {
        dir.create(elt, showWarnings = FALSE, recursive = TRUE)
    }

    exists <- dir.exists(path)

    if (all(exists)) {
        # Nothing left to do if the missing ones were successfully created
        return(invisible())
    }

    path <- path[!exists]
    path <- encodeString(path, quote = "\"")
    path <- paste0(path, collapse = ", ")

    stop("Can't create the directory at: ", path, call. = FALSE)
}

ensure_parent_directory <- function(path) {
    ensure_directory(dirname(path))
}

ensure_file <- function(path) {
    exists <- file.exists(path)

    if (all(exists)) {
        # All files exist already, nothing to do
        return(invisible())
    }

    path <- path[!exists]

    # Create parent directories as needed
    ensure_parent_directory(path)

    # Try to create the missing files
    file.create(path, showWarnings = FALSE)

    exists <- file.exists(path)

    if (all(exists)) {
        # We successfully created the new files and can detect
        # their existance
        return(invisible())
    }

    path <- path[!exists]
    path <- encodeString(path, quote = "\"")
    path <- paste0(path, collapse = ", ")

    stop("Can't create the files at: ", path, call. = FALSE)
}

vec_paste0 <- function(..., collapse = NULL) {
    # Like `paste0()`, but avoids `paste0("prefix:", character())`
    # resulting in `"prefix:"` and instead recycles to size 0.
    # Assumes that inputs with size >0 would validly recycle to size 0.
    args <- list(...)

    if (any(lengths(args) == 0L)) {
        character()
    } else {
        args <- c(args, list(collapse = collapse))
        do.call(paste0, args)
    }
}

# Meant for debugging inside lldb. See `push_rds()` util on the Rust side.
push_rds <- function(x, path = NULL, context = "") {
    if (is.null(path)) {
        path <- Sys.getenv("RUST_PUSH_RDS_PATH")
        if (!nzchar(path)) {
            stop("Must provide path or set `RUST_PUSH_RDS_PATH`")
        }
    }

    if (file.exists(path)) {
        xs <- readRDS(path)
        stopifnot(
            is.data.frame(xs),
            "POSIXct" %in% class(xs$date),
            is.character(xs$context),
            is.list(xs$x)
        )
    } else {
        xs <- tibble::tibble(
            date = as.POSIXct(NULL),
            context = character(),
            x = list()
        )
    }

    x <- tibble::tibble(
        date = Sys.time(),
        context = context,
        x = list(x)
    )
    xs <- rbind(x, xs)

    saveRDS(xs, path)
    xs
}

is_string <- function(x) {
    is.character(x) && length(x) == 1 && !is.na(x)
}

local_options <- function(..., .frame = parent.frame()) {
    options <- list(...)
    old <- options(options)
    defer(options(old), envir = .frame)
    invisible(old)
}

#' A Positron specific temporary directory
#'
#' Creates a directory at `tempdir()` + `positron/` + `...`, or dies trying.
#'
#' @param ... Further subdirectories to create
#'
#' @noRd
positron_tempdir <- function(...) {
    dir <- tempdir()
    out <- file.path(dir, "positron", ...)

    if (!dir.exists(out)) {
        if (!dir.create(out, showWarnings = FALSE, recursive = TRUE)) {
            stop(sprintf("Can't create temporary directory at '%s'.", out))
        }
    }

    out
}
