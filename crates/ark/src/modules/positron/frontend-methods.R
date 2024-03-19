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
.ps.ui.navigateToFile <- function(file, line, column) {
    .ps.Call("ps_ui_navigate_to_file", file, line, column)
}

#' @export
.ps.ui.executeCommand <- function(command) {
    .ps.Call("ps_ui_execute_command", command)
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
.ps.ui.debugSleep <- function(ms) {
    # stopifnot(is.numeric(ms) && length(ms) == 1 && !is.na(ms))
    .ps.Call("ps_ui_debug_sleep", ms)
}
