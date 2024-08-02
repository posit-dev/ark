#
# frontend-methods.R
#
# Copyright (C) 2024 Posit Software, PBC. All rights reserved.
#
#

# TODO: Unexport these methods

#' @export
.ps.ui.LastActiveEditorContext <- function() {
    .ps.Call("ps_ui_last_active_editor_context")
}

#' @export
.ps.ui.setSelectionRanges <- function(ranges) {
    .ps.Call("ps_ui_set_selection_ranges", ranges)
}

#' @export
.ps.ui.modifyEditorSelections <- function(ranges, values) {
    .ps.Call("ps_ui_modify_editor_selections", ranges, values)
}

#' @export
.ps.ui.workspaceFolder <- function() {
    .ps.Call("ps_ui_workspace_folder")
}

#' @export
.ps.ui.openWorkspace <- function(path, newSession) {
    .ps.Call("ps_ui_open_workspace", path, newSession)
}

#' @export
.ps.ui.navigateToFile <- function(file = character(0), line = -1L, column = -1L) {
    file <- normalizePath(file)
    .ps.Call("ps_ui_navigate_to_file", file, line, column)
}

#' @export
.ps.ui.newDocument <- function(contents, languageId) {
    .ps.Call("ps_ui_new_document", contents, languageId)
}

#' @export
.ps.ui.executeCommand <- function(command) {
    .ps.Call("ps_ui_execute_command", command)
}

#' @export
.ps.ui.executeCommandAwait <- function(command) {
    .ps.Call("ps_ui_execute_command_await", command)
}

#' @export
.ps.ui.executeCode <- function(code, focus) {
    .ps.Call("ps_ui_execute_code", code, focus)
}

#' @export
.ps.ui.showMessage <- function(message) {
    .ps.Call("ps_ui_show_message", message)
}

#' @export
.ps.ui.showDialog <- function(title, message) {
    .ps.Call("ps_ui_show_dialog", title, message)
}

#' @export
.ps.ui.showQuestion <- function(title, message, ok, cancel) {
    .ps.Call("ps_ui_show_question", title, message, ok, cancel)
}

#' @export
.ps.ui.showUrl <- function(url) {
    .ps.Call("ps_ui_show_url", url)
}

#' @export
.ps.ui.evaluateWhenClause <- function(whenClause) {
    .ps.Call("ps_ui_evaluate_when_clause", whenClause)
}

#' @export
.ps.ui.debugSleep <- function(ms) {
    # stopifnot(is.numeric(ms) && length(ms) == 1 && !is.na(ms))
    .ps.Call("ps_ui_debug_sleep", ms)
}
