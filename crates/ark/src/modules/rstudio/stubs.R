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

    file <- normalizePath(file)
    invisible(.ps.ui.navigateToFile(file, line, column))
}

#' @export
.rs.api.restartSession <- function(command = "") {
    # TODO: support followup `command` argument
    stopifnot(command == "")

    invisible(.ps.ui.executeCommand('workbench.action.languageRuntime.restart'))
}
