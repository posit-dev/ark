#
# options.R
#
# Copyright (C) 2023-2025 Posit Software, PBC. All rights reserved.
#
#

# Avoid overwhelming the console
options(max.print = 1000)

# Enable HTML help
options(help_type = "html")

# Use internal editor
options(editor = function(file, title, ..., name = NULL) {
    handler_editor(file = file, title = title, ..., name = name)
})

# Use custom browser implementation
options(browser = function(url) {
    .ps.Call("ps_browse_url", as.character(url))
})

# Register our password handler as the generic `askpass` option.
# Same as RStudio, see `?rstudioapi::askForPassword` for rationale.
options(askpass = function(prompt) {
    .ps.ui.askForPassword(prompt)
})

# Show Plumber apps in the viewer
options(plumber.docs.callback = function(url) {
    .ps.ui.showUrl(url)
})

# Show Shiny applications in the viewer
options(shiny.launch.browser = function(url) {
    .ps.ui.showUrl(url)
})

# Show Profvis output in the viewer
options(profvis.print = function(x) {
    # Render the widget to a tag list to create standalone HTML output.
    # (htmltools is a Profvis dependency so it's guaranteed to be available)
    rendered <- htmltools::as.tags(x, standalone = TRUE)

    # Render the HTML content to a temporary file
    tmp_file <- htmltools::html_print(rendered, viewer = NULL)

    # Pass the file to the viewer
    .ps.Call("ps_html_viewer", tmp_file, "R Profile", -1L, "editor")
})
