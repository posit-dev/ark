#
# viewer.R
#
# Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
#
#

options("viewer" = function(url, height = NULL, ...) {
    # Validate the URL argument.
    if (!is.character(url) || (length(url) != 1))
        stop("url must be a single element character vector.")

    # Normalize paths for comparison. This is necessary because on e.g. macOS,
    # the `tempdir()` may contain `//` or other non-standard path separators.
    normalizedPath <- normalizePath(url, mustWork = FALSE)
    normalizedTempdir <- normalizePath(tempdir(), mustWork = FALSE)

    # Validate the height argument.
    height <- .ps.validate.viewer.height(height)

    # Is the URL a temporary file?
    if (startsWith(normalizedPath, normalizedTempdir)) {
        # Derive a title for the viewer from the path.
        title <- .ps.viewer.title(normalizedPath)

        # If so, open it in the HTML viewer.
        .ps.Call("ps_html_viewer", normalizedPath, title, height, FALSE)
    } else {
        # If not, open it in the system browser.
        utils::browseURL(normalizedPath, ...)
    }
})
