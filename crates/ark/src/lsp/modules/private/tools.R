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
    dir.create(path, showWarnings = FALSE, recursive = TRUE)
}

ensure_parent_directory <- function(path) {
    ensure_directory(dirname(path))
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
