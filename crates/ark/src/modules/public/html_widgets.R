#
# html_widgets.R
#
# Copyright (C) 2023 Posit Software, PBC. All rights reserved.
#
#
.ps.view_html_widget <- function(x, ...) {
    # todo:
    #
    # use htmltools::as.tags(standalone = TRUE) to convert the htmlwidget to a
    # list of tags
    #
    # note this is a list that needs as.character to convert to html
    #
    # then use htmltools::resolveDependencies() to get the list of dependencies

    .ps.Call("ps_html_widget",
        class(x)[1],
        htmltools::renderTags(x))
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
