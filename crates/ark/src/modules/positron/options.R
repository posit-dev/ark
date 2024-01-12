#
# options.R
#
# Copyright (C) 2023 Posit Software, PBC. All rights reserved.
#
#

# Avoid overwhelming the console
options(max.print = 1000)

# Enable HTML help
options(help_type = "html")

# Use internal editor
options(editor = function(file, title, ..., name = NULL) {
    handler_editor(file = file, title = title, ..., name = name)
})

# Use custom browser implementation
options(browser = function(url) {
    .ps.Call("ps_browse_url", as.character(url))
})

# Set up graphics device
options(device = function() {
    .ps.Call("ps_graphics_device")
})

# Set cran mirror
repos <- getOption("repos")
rstudio_cran <- "https://cran.rstudio.com/"

if (is.null(repos) || !is.character(repos)) {
    options(repos = c(CRAN = rstudio_cran))
} else {
    if ("CRAN" %in% names(repos)) {
        if (identical(repos[["CRAN"]], "@CRAN@")) {
            repos[["CRAN"]] <- rstudio_cran
            options(repos = repos)
        }
    } else {
        repos <- c(CRAN = rstudio_cran, repos)
        options(repos = repos)
    }
}

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
