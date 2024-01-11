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

    # Resolve the dependencies for the widget (scripts, stylesheets, etc.).
    dependencies <- htmltools::resolveDependencies(
        attr(rendered, "html_dependencies", exact = TRUE))

    # Pass the widget to the viewer. Positron will assemble the final HTML
    # document from these components.
    .ps.Call("ps_html_widget",
        class(x)[1],
        list(
            tags = rendered,
            dependencies = dependencies,
            sizing_policy = x$sizingPolicy))
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
