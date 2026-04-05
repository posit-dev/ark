#
# options.R
#
# Copyright (C) 2023-2025 Posit Software, PBC. All rights reserved.
#
#

# Called from Rust after sourcing the user's Rprofile so that user-defined
# options take precedence over our defaults.
initialize_options <- function() {
    # These options have non-NULL defaults in R, so we can't detect user
    # overrides by checking for NULL. They are always set unless the user
    # has listed the option name in `ark.protected_options`.

    # Use Positron editor
    set_override(
        "editor",
        function(file, title, ..., name = NULL) {
            handler_editor(file = file, title = title, ..., name = name)
        }
    )

    # Use Positron viewer to browse URLs
    set_override(
        "browser",
        function(url) {
            .ps.Call("ps_browse_url", as.character(url))
        }
    )

    # Register our password handler as the generic `askpass` option.
    # Same as RStudio, see `?rstudioapi::askForPassword` for rationale.
    set_override(
        "askpass",
        function(prompt) {
            .ps.ui.askForPassword(prompt)
        }
    )

    set_override("connectionObserver", .ps.connection_observer())

    # Declare the function name that `dev.new()` and `GECurrentDevice()`
    # go looking for to create a new graphics device when the current one
    # is `"null device"` and a new plot is requested
    set_override("device", ARK_GRAPHICS_DEVICE_NAME)

    # Avoid overwhelming the console
    set_override("max.print", 1000)

    # These options default to NULL in R, so a non-NULL value means the
    # user has set them. They are only set when the current value is NULL,
    # unless the user has also listed them in `ark.protected_options`,
    # which allows the user to preserve the default `NULL` value.

    # Enable HTML help
    set_default("help_type", "html")

    set_default("viewer", viewer_option_handler)

    # Show Shiny applications in the viewer. This is technically redundant with
    # the `browser` override since Shiny calls `browseURL()` by default in
    # interactive sessions.
    set_default(
        "shiny.launch.browser",
        function(url) {
            .ps.ui.showUrl(url)
        }
    )

    # Show Plumber apps in the viewer
    set_default(
        "plumber.docs.callback",
        function(url) {
            .ps.ui.showUrl(url)
        }
    )

    # Show Profvis output in the viewer
    set_default(
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

is_protected <- function(name) {
    name %in% getOption("ark.protected_options", default = character())
}

# Set an option unconditionally, unless listed in `ark.protected_options`
set_override <- function(name, value) {
    if (is_protected(name)) {
        return(invisible())
    }
    do.call(options, set_names(list(value), name))
}

# Set an option only when currently `NULL`, unless listed in `ark.protected_options`
set_default <- function(name, value) {
    if (is_protected(name)) {
        return(invisible())
    }
    if (!is.null(getOption(name))) {
        return(invisible())
    }
    do.call(options, set_names(list(value), name))
}
