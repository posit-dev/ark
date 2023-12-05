#
# context.R
#
# Copyright (C) 2023 Posit Software, PBC. All rights reserved.
#
#

.ps.getActiveDocumentContext <- function() {
    .ps.Call("ps_context_active_document")
}
