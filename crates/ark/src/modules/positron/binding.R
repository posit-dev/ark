#
# binding.R
#
# Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
#
#

#' @export
.ps.binding.replace <- function(symbol, replacement, envir) {
    local_unlock_binding(envir, symbol)

    original <- envir[[symbol]]
    assign(symbol, replacement, envir = envir)
    invisible(original)
}

local_unlock_binding <- function(env, sym, frame = parent.frame()) {
    if (bindingIsLocked(sym, env)) {
        unlockBinding(sym, env)
        defer(lockBinding(sym, env), envir = frame)
    }
}
