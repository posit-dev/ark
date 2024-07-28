#' @export
.rs.api.executeCommand <- function(commandId, quiet = FALSE) {
    commandId <- switch(
        commandId,
        "activateConsole" = "workbench.action.positronConsole.focusConsole",
        "activateTerminal" = "workbench.action.terminal.focus",
        # This command includes untitled files in RStudio:
        "saveAllSourceDocs" = "workbench.action.files.saveAll",
        # This command is a silent no-op in RStudio when there is no git repo:
        "vcsRefresh" = {
            if (.ps.ui.evaluateWhenClause("gitOpenRepositoryCount >= 1")) {
                "git.refresh"
            } else {
                return()
            }
        },
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
