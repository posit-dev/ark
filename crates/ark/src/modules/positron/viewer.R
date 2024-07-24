#
# viewer.R
#
# Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
#
#

options("viewer" = function(url, height = NULL, ...) {
    # Is the URL a temporary file?
    if (startsWith(url, tempdir())) {
        # Use the filename as the label, unless it's an index file, in which
        # case use the directory name.
        fname <- tolower(basename(url))
        if (identical(fname, "index.html") || identical(fname, "index.htm")) {
            fname <- basename(dirname(url))
        }
        # If so, open it in the HTML viewer.
        .ps.Call("ps_html_viewer", url, fname, height, FALSE)
    } else {
        # If not, open it in the system browser.
        utils::browseURL(url, ...)
    }
})
