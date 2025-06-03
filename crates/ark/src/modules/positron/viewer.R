#
# viewer.R
#
# Copyright (C) 2023-2025 Posit Software, PBC. All rights reserved.
#
#

options("viewer" = function(url, height = NULL, ...) {
    # Validate the URL argument.
    if (!is_string(url)) {
        stop("`url` must be a string.")
    }

    # Validate the height argument.
    height <- .ps.validate.viewer.height(height)

    # Open `http(s)://` urls in the browser immediately, avoid normalizing their
    # paths since they aren't files (posit-dev/positron#4843)
    if (is_http_url(url)) {
        return(utils::browseURL(url, ...))
    }

    # Normalize file paths for comparison against the `tempdir()`. This is
    # necessary because on e.g. macOS, the `tempdir()` may contain `//` or other
    # non-standard path separators.
    normalizedPath <- normalizePath(url, mustWork = FALSE)
    normalizedTempdir <- normalizePath(tempdir(), mustWork = FALSE)

    # Is the URL a temporary file?
    if (startsWith(normalizedPath, normalizedTempdir)) {
        # Derive a title for the viewer from the path.
        title <- .ps.viewer.title(normalizedPath)

        # If so, open it in the HTML viewer.
        .ps.Call("ps_html_viewer", normalizedPath, title, height, FALSE)
    } else {
        # If not, fall back to opening it in the system browser.
        utils::browseURL(normalizedPath, ...)
    }
})
