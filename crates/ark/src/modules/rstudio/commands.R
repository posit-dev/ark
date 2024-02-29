#' @export
.rs.api.executeCommand <- function(commandId, quiet = FALSE) {
    commandId <- switch(
        commandId,
        "activateConsole" = "workbench.action.positronConsole.focusConsole",
        {
            if (!quiet) stop("This command is not yet supported in Positron.")
            return()
        }
    )
    .ps.ui.executeCommand(commandId)
}
