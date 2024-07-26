#' @export
.rs.api.getActiveProject <- function() {
    invisible(.ps.ui.workspaceFolder())
}

#' @export
.rs.api.navigateToFile <- function(file = character(0),
                                   line = -1L,
                                   column = -1L,
                                   moveCursor = TRUE) {
    # TODO: support moveCursor argument
    stopifnot(moveCursor)

    invisible(.ps.ui.navigateToFile(file, line, column))
}

#' @export
.rs.api.sendToConsole <- function(code, echo = TRUE, execute = TRUE, focus = TRUE, animate = FALSE) {
    # TODO: support other args
    stopifnot(echo && execute && !animate)

    # If we add new args later, remember to put them **after** the existing args
    invisible(.ps.ui.executeCode(paste(code, collapse = "\n"), focus))
}

#' @export
.rs.api.restartSession <- function(command = "") {
    # TODO: support followup `command` argument
    stopifnot(command == "")

    invisible(.ps.ui.executeCommand('workbench.action.languageRuntime.restart'))
}

#' @export
.rs.api.openProject <- function(path = NULL, newSession = FALSE) {
    path <- normalizePath(path)
    invisible(.ps.ui.openWorkspace(path, newSession))
}

#' @export
.rs.api.viewer <- function (url, height = NULL) {
    # Validate arguments
    if (!is.character(url) || (length(url) != 1))
        stop("url must be a single element character vector.")
    height <- .ps.validate.viewer.height(height)

    # Derive a title for the viewer from the path.
    title <- .ps.viewer.title(normalizedPath)

    invisible(.Call("ps_html_viewer",
        url,     # The URL of the file to view
        fname,   # The name of the file to display in the viewer
        height,  # The desired height
        FALSE,   # Whether the object is a plot; guess FALSE
        PACKAGE = "(embedding)"))
}
