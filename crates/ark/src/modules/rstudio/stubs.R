#' @export
.rs.api.navigateToFile <- function(file = character(0),
                                   line = -1L,
                                   column = -1L,
                                   moveCursor = TRUE) {

    # TODO: support line, column, moveCursor arguments
    .ps.ui.executeCommand('vscode.open', list(uriOrString = file))
}

#' @export
.rs.api.restartSession <- function(command = "") {
    # TODO: support followup `command` argument
    invisible(.ps.ui.executeCommand('workbench.action.languageRuntime.restart'))
}
