#
# frontend-methods.R
#
# Copyright (C) 2024 Posit Software, PBC. All rights reserved.
#
#

.ps.frontend.LastActiveEditorContext <- function() {
    .ps.Call("ps_frontend_last_active_editor_context")
}

.ps.frontend.debugSleep <- function(ms) {
    stopifnot(is.numeric(ms) && length(ms) == 1 && !is.na(ms))
    .ps.Call("ps_frontend_debug_sleep", ms)
}
