#' @export
.rs.api.executeCommand <- function(commandId, quiet = FALSE) {
    commandId <- switch(
        commandId,
        "activateConsole" = "workbench.action.positronConsole.focusConsole",
        "activateTerminal" = "workbench.action.terminal.focus",
        # This command includes untitled files in RStudio:
        "saveAllSourceDocs" = "workbench.action.files.saveAll",
        # https://github.com/posit-dev/positron/issues/2697
        # This command is a silent no-op in RStudio when there is no git repo:
        "vcsRefresh" = {
            if (.ps.ui.evaluateWhenClause("config.git.enabled && gitOpenRepositoryCount > 0")) {
                "git.refresh"
            } else {
                return(NULL)
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
