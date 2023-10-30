#
# viewer.R
#
# Copyright (C) 2023 Posit Software, PBC. All rights reserved.
#
#

options("viewer" = function(url) {
    # Is the URL a temporary file?
    if (startsWith(url, tempdir())) {
        # If so, open it in the HTML viewer.
        .ps.Call("ps_html_viewer", url)
    } else {
        # If not, open it in the system browser.
        system2("open", url)
    }
})
