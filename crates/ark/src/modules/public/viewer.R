#
# viewer.R
#
# Copyright (C) 2023 Posit Software, PBC. All rights reserved.
#
#

options("viewer" = function(url, ...) {
    # Is the URL a temporary file?
    if (startsWith(url, tempdir())) {
        # If so, open it in the HTML viewer.
        .ps.Call("ps_html_viewer", url)
    } else {
        # If not, open it in the system browser.
        utils::browseURL(url, ...)
    }
})

.ps.view_html_widget <- function(x, ...) {
    print(paste0("HTML WIDGET: ", class(x)))
}

.ps.viewer.addOverrides <- function() {
    .ps.s3.addS3Override("print.htmlwidget", .ps.view_html_widget)
}

.ps.viewer.removeOverrides <- function() {
    .ps.s3.removeS3Override("print.htmlwidget")
}

# When the htmlwidgets package is loaded, inject/overlay our print method.
loadEvent <- packageEvent("htmlwidgets", "onLoad")
setHook(loadEvent, function(...) {
   .ps.viewer.addOverrides()
}, action = "append")

unloadEvent <- packageEvent("htmlwidgets", "onUnload")
setHook(unloadEvent, function(...) {
   .ps.viewer.removeOverrides()
}, action = "append")
