#
# frontend-methods.R
#
# Copyright (C) 2024 Posit Software, PBC. All rights reserved.
#
#

.ps.ui.LastActiveEditorContext <- function() {
    .ps.Call("ps_ui_last_active_editor_context")
}

.ps.ui.debugSleep <- function(ms) {
    # stopifnot(is.numeric(ms) && length(ms) == 1 && !is.na(ms))
    .ps.Call("ps_ui_debug_sleep", ms)
}
