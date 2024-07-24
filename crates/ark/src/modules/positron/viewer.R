#
# viewer.R
#
# Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
#
#

options("viewer" = function(url, height = NULL, ...) {
    # Normalize paths for comparison. This is necessary because on e.g. macOS,
    # the `tempdir()` may contain `//` or other non-standard path separators.
    normalizedPath <- normalizePath(url, mustWork = FALSE)
    normalizedTempdir <- normalizePath(tempdir(), mustWork = FALSE)

    # Is the URL a temporary file?
    if (startsWith(normalizedPath, normalizedTempdir)) {
        # Use the filename as the label, unless it's an index file, in which
        # case use the directory name.
        fname <- tolower(basename(url))
        if (identical(fname, "index.html") || identical(fname, "index.htm")) {
            fname <- basename(dirname(url))
        }

        # R HTML widgets get printed to temporary files starting with the name
        # "viewhtml". This makes an ugly label, so we give it a nicer one.
        if (startsWith(fname, "viewhtml")) {
            fname <- "R HTML widget"
        }

        # If so, open it in the HTML viewer.
        .ps.Call("ps_html_viewer", url, fname, height, FALSE)
    } else {
        # If not, open it in the system browser.
        utils::browseURL(url, ...)
    }
})
