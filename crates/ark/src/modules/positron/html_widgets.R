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

    # Render the tag list to a temporary file using html_print. Don't view the
    # file yet; we'll do that in a bit.
    tmp_file <- htmltools::html_print(rendered, viewer = NULL)

    # Guess whether this is a plot-like widget based on its sizing policy.
    is_plot <- isTRUE(x$sizingPolicy$knitr$figure)

    # Derive the height of the viewer pane from the sizing policy of the widget.
    height <- .ps.validate.viewer.height(x$sizingPolicy$viewer$paneHeight)

    # Attempt to derive a label for the widget from its class. If the class is
    # empty, use a default label.
    label <- class(x)[1]
    if (nzchar(label)) {
        label <- paste(label, "HTML widget")
    } else {
        label <- "R HTML widget"
    }

    # Pass the widget to the viewer. Positron will assemble the final HTML
    # document from these components.
    .ps.Call("ps_html_viewer",
        tmp_file,
        label,
        height,
        is_plot)
}

#' @export
.ps.viewer.addOverrides <- function() {
    add_s3_override("print.htmlwidget", .ps.view_html_widget)
    add_s3_override("print.shiny.tag", .ps.view_html_widget)
    add_s3_override("print.shiny.tag.list", .ps.view_html_widget)
}

#' @export
.ps.viewer.removeOverrides <- function() {
    remove_s3_override("print.htmlwidget")
    remove_s3_override("print.shiny.tag")
    remove_s3_override("print.shiny.tag.list")
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

# Validate the height argument for the viewer function; returns an
# integer or stops with an error.
.ps.validate.viewer.height <- function(height) {
    if (identical(height, "maximize"))
        # The height of the viewer pane is set to -1 to maximize it.
        height <- -1L
    if (!is.null(height) && (!is.numeric(height) || (length(height) !=
        1)))
        stop("Invalid height: ",
            height,
            "Must be a single element numeric vector or 'maximize'.")
    if (is.null(height)) {
        # The height of the viewer pane is set to 0 to signal that
        # no specific height is requested.
        height <- 0L
    }
    as.integer(height)
}

# Derive a title for the viewer from the given file path
.ps.viewer.title <- function(path) {
    # Use the filename as the label, unless it's an index file, in which
    # case use the directory name.
    fname <- tolower(basename(path))
    if (identical(fname, "index.html") || identical(fname, "index.htm")) {
        fname <- basename(dirname(path))
    }

    # R HTML widgets get printed to temporary files starting with the name
    # "viewhtml". This makes an ugly label, so we give it a nicer one.
    if (startsWith(fname, "viewhtml")) {
        "R HTML widget"
    } else {
        fname
    }
}
