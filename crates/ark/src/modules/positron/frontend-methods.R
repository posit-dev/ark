#
# frontend-methods.R
#
# Copyright (C) 2024 Posit Software, PBC. All rights reserved.
#
#

#' @export
.ps.ui.LastActiveEditorContext <- function() {
    .ps.Call("ps_ui_last_active_editor_context")
}

#' @export
.ps.ui.documentNew <- function(contents, languageId, character, line) {
    .ps.Call("ps_ui_document_new", contents, languageId, character, line)
}

#' @export
.ps.ui.navigateToFile <- function(file) {
    .ps.Call("ps_ui_navigate_to_file", file)
}

#' @export
.ps.ui.executeCommand <- function(command) {
    .ps.Call("ps_ui_execute_command", command)
}

#' @export
.ps.ui.showMessage <- function(message) {
    .ps.Call("ps_show_message", message)
}

#' @export
.ps.ui.debugSleep <- function(ms) {
    # stopifnot(is.numeric(ms) && length(ms) == 1 && !is.na(ms))
    .ps.Call("ps_ui_debug_sleep", ms)
}
