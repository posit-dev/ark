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
.ps.ui.executeCommand <- function(command, args = list()) {
    .ps.Call("ps_ui_execute_command", command, args)
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
