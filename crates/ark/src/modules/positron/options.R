#
# options.R
#
# Copyright (C) 2023-2025 Posit Software, PBC. All rights reserved.
#
#

# Called from Rust after sourcing the user's Rprofile so that user-defined
# options take precedence over our defaults.
initialize_options <- function() {
    # Core Positron integration, always set
    options(editor = function(file, title, ..., name = NULL) {
        handler_editor(file = file, title = title, ..., name = name)
    })

    options(browser = function(url) {
        .ps.Call("ps_browse_url", as.character(url))
    })

    options(askpass = function(prompt) {
        .ps.ui.askForPassword(prompt)
    })

    # Declare the function name that `dev.new()` and `GECurrentDevice()`
    # go looking for to create a new graphics device when the current one
    # is `"null device"` and a new plot is requested
    options(device = ARK_GRAPHICS_DEVICE_NAME)

    options(connectionObserver = .ps.connection_observer())

    # Only override when the user hasn't set them in their Rprofile.
    # `max.print` defaults to 99999L in R.
    if (identical(getOption("max.print"), 99999L)) {
        options(max.print = 1000)
    }

    if (is.null(getOption("help_type"))) {
        options(help_type = "html")
    }

    if (is.null(getOption("viewer"))) {
        options(viewer = viewer_option_handler)
    }

    if (is.null(getOption("shiny.launch.browser"))) {
        options(shiny.launch.browser = function(url) {
            .ps.ui.showUrl(url)
        })
    }

    if (is.null(getOption("plumber.docs.callback"))) {
        options(plumber.docs.callback = function(url) {
            .ps.ui.showUrl(url)
        })
    }

    if (is.null(getOption("profvis.print"))) {
        options(profvis.print = function(x) {
            # Render the widget to a tag list to create standalone HTML output.
            # (htmltools is a Profvis dependency so it's guaranteed to be available)
            rendered <- htmltools::as.tags(x, standalone = TRUE)

            # Render the HTML content to a temporary file
            tmp_file <- htmltools::html_print(rendered, viewer = NULL)

            # Pass the file to the viewer
            .ps.Call("ps_html_viewer", tmp_file, "R Profile", -1L, "editor")
        })
    }
}
