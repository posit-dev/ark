#' @export
.rs.api.executeCommand <- function(commandId, quiet = FALSE) {
    commandId <- switch(
        commandId,
        "activateConsole" = "workbench.action.positronConsole.focusConsole",
        "activateTerminal" = "workbench.action.terminal.focus",
        "saveAllSourceDocs" = "workbench.action.files.saveAll",
        {
            if (!quiet) .ps.ui.showMessage(paste0("The command '", commandId, "' does not exist."))
            return()
        }
    )
    .ps.ui.executeCommand(commandId)
}
