#
# options.R
#
# Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
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

# Set up graphics device
options(device = function() {
    .ps.Call("ps_graphics_device")
})

# Set cran mirror
repos <- getOption("repos")
rstudio_cran <- "https://cran.rstudio.com/"
if (is.null(repos) || !is.character(repos)) {
    repos <- c(CRAN = rstudio_cran)
} else {
    if ("CRAN" %in% names(repos)) {
        if (identical(repos[["CRAN"]], "@CRAN@")) {
            repos[["CRAN"]] <- rstudio_cran
            attr(repos, "IDE") <- TRUE
        }
    } else {
        repos <- c(CRAN = rstudio_cran, repos)
    }
}
options(repos = repos)

# Show Plumber apps in the viewer
options(plumber.docs.callback = function(url) {
    .ps.ui.showUrl(url)
})

# Show Shiny applications in the viewer
options(shiny.launch.browser = function(url) {
    .ps.ui.showUrl(url)
})
