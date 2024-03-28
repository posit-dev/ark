#' @export
.rs.api.executeCommand <- function(commandId, quiet = FALSE) {
    commandId <- switch(
        commandId,
        "activateConsole" = "workbench.action.positronConsole.focusConsole",
        "activateTerminal" = "workbench.action.terminal.focus",
        # This command includes untitled files in RStudio:
        "saveAllSourceDocs" = "workbench.action.files.saveAll",
        "vcsRefresh" = "git.refresh",
        "refreshFiles" = "workbench.files.action.refreshFilesExplorer",
        {
            if (!quiet) {
                .ps.ui.showMessage(paste0("The command '", commandId, "' does not exist."))
            }
            return()
        }
    )
    .ps.ui.executeCommand(commandId)
}
