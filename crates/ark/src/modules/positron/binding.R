#
# binding.R
#
# Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
#
#

#' @export
.ps.binding.replace <- function(symbol, replacement, envir) {

    if (bindingIsLocked(symbol, envir)) {
        unlockBinding(symbol, envir)
        on.exit(lockBinding(symbol, envir), add = TRUE)
    }

    original <- envir[[symbol]]
    assign(symbol, replacement, envir = envir)
    invisible(original)

}
