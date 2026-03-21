#
# options.R
#
# Copyright (C) 2023-2025 Posit Software, PBC. All rights reserved.
#
#

# Called from Rust after sourcing the user's Rprofile so that user-defined
# options take precedence over our defaults.
initialize_options <- function() {
    # Core Positron integration, always set unless the user has protected
    # the option with `I()` in their Rprofile

    # Use Positron editor
    set_unless_asis(
        "editor",
        function(file, title, ..., name = NULL) {
            handler_editor(file = file, title = title, ..., name = name)
        }
    )

    # Use Positron viewer to browse URLs
    set_unless_asis(
        "browser",
        function(url) {
            .ps.Call("ps_browse_url", as.character(url))
        }
    )

    # Register our password handler as the generic `askpass` option.
    # Same as RStudio, see `?rstudioapi::askForPassword` for rationale.
    set_unless_asis(
        "askpass",
        function(prompt) {
            .ps.ui.askForPassword(prompt)
        }
    )

    set_unless_asis("connectionObserver", .ps.connection_observer())

    # Declare the function name that `dev.new()` and `GECurrentDevice()`
    # go looking for to create a new graphics device when the current one
    # is `"null device"` and a new plot is requested
    set_unless_asis("device", ARK_GRAPHICS_DEVICE_NAME)

    # Avoid overwhelming the console
    set_unless_asis("max.print", 1000)

    # Only override the following options if they are set to NULL

    # Enable HTML help
    set_when_null("help_type", "html")

    set_when_null("viewer", viewer_option_handler)

    # Show Shiny applications in the viewer
    set_when_null(
        "shiny.launch.browser",
        function(url) {
            .ps.ui.showUrl(url)
        }
    )

    # Show Plumber apps in the viewer
    set_when_null(
        "plumber.docs.callback",
        function(url) {
            .ps.ui.showUrl(url)
        }
    )

    # Show Profvis output in the viewer
    set_when_null(
        "profvis.print",
        function(x) {
            # Render the widget to a tag list to create standalone HTML output.
            # (htmltools is a Profvis dependency so it's guaranteed to be available)
            rendered <- htmltools::as.tags(x, standalone = TRUE)

            # Render the HTML content to a temporary file
            tmp_file <- htmltools::html_print(rendered, viewer = NULL)

            # Pass the file to the viewer
            .ps.Call("ps_html_viewer", tmp_file, "R Profile", -1L, "editor")
        }
    )
}

# Set an option unless the user has protected it with `I()` in their Rprofile
set_unless_asis <- function(name, value) {
    current <- getOption(name)
    if (inherits(current, "AsIs")) {
        # Strip the `AsIs` class so it doesn't interfere with normal usage
        do.call(options, set_names(list(unclass(current)), name))
        return(invisible())
    }
    do.call(options, set_names(list(value), name))
}

# Set an option only when it is currently NULL (i.e. the user hasn't set it)
set_when_null <- function(name, value) {
    if (!is.null(getOption(name))) {
        return(invisible())
    }
    do.call(options, set_names(list(value), name))
}
