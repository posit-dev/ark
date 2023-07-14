#
# options.R
#
# Copyright (C) 2023 Posit Software, PBC. All rights reserved.
#
#

handler_editor <- function(file, title, ..., name = NULL) {
    # `file.edit()` calls this as `editor(file = file, title = title)`
    # `edit()` calls this as `editor(name = name, file = file, title = title)`

    if (!is.null(name)) {
        stop("Editing objects is not currently supported.", call. = FALSE)
    }

    if (identical(file, "")) {
        # i.e. `edit()` with no arguments. Also `file.edit("")`.
        # Opens a temporary file for editing.
        file <- tempfile(fileext = ".txt")
    }

    file <- as.character(file)
    title <- as.character(title)

    # Get absolute path to file
    file <- normalizePath(file, mustWork = FALSE)

    # Make sure the requested files exist, creating them if they don't
    ensure_file(file)

    # Edit those files.
    .ps.Call("ps_editor", file, title)

    invisible()
}
