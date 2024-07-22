#
# html_widgets.R
#
# Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
#
#
#' @export
.ps.view_html_widget <- function(x, ...) {
    # Render the widget to a tag list.
    rendered <- htmltools::as.tags(x, standalone = TRUE)

    # Render the tag list to a temproary file using html_print. Don't view the
    # file yet; we'll do that in a bit.
    tmp_file <- htmltools::html_print(rendered, viewer = NULL)

    # Guess whether this is a plot or plot-like object. The default is to treat
    # figures as plots, but if the viewer pane height is set to 'maximize', then
    # we treat it as a non-plot object.
    is_plot <- x$sizingPolicy$knitr$figure
    if (identical(x$sizingPolicy$viewer$paneHeight, 'maximize')) {
        is_plot <- FALSE
    }

    # Pass the widget to the viewer. Positron will assemble the final HTML
    # document from these components.
    .ps.Call("ps_html_viewer",
        tmp_file,
        class(x)[1],
        is_plot)
}

#' @export
.ps.viewer.addOverrides <- function() {
    add_s3_override("print.htmlwidget", .ps.view_html_widget)
}

#' @export
.ps.viewer.removeOverrides <- function() {
    remove_s3_override("print.htmlwidget")
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
