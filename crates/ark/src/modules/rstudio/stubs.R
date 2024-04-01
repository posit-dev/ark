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
