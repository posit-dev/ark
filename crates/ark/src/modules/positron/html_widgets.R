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

    # Derive the height of the viewer pane from the sizing policy of the widget.
    is_plot <- x$sizingPolicy$knitr$figure
    height <- x$sizingPolicy$viewer$paneHeight
    if (identical(height, 'maximize')) {
        height <- -1
    } else if (is.null(height)) {
        height <- 0
    }

    # Pass the widget to the viewer. Positron will assemble the final HTML
    # document from these components.
    .ps.Call("ps_html_viewer",
        tmp_file,
        class(x)[1],
        height,
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
