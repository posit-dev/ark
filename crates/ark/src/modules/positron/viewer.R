#
# viewer.R
#
# Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
#
#

options("viewer" = function(url, height = NULL, ...) {
    # Is the URL a temporary file?
    if (startsWith(url, tempdir())) {
        # If so, open it in the HTML viewer.
        .ps.Call("ps_html_viewer", url)
        # TODO: handle `height` for HTML viewer
    } else {
        # If not, open it in the system browser.
        utils::browseURL(url, ...)
    }
})
