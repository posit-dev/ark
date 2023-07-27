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

# Checks if a package is installed without loading it.
# Could be slow on network drives.
is_installed <- function(pkg, minimum_version = NULL) {
    installed <- system.file(package = pkg) != ""

    if (installed && !is.null(minimum_version)) {
        installed <- packageVersion(pkg) >= minimum_version
    }

    installed
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
