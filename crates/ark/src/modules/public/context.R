#
# context.R
#
# Copyright (C) 2024 Posit Software, PBC. All rights reserved.
#
#

.ps.getActiveDocumentContext <- function() {
    .ps.Call("ps_get_context_active_document")
}
