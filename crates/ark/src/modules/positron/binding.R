#
# binding.R
#
# Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
#
#

env_bind_force <- function(env, name, value) {
    name <- as.character(name)

    local_unlock_binding(env, name)

    original <- env[[name]]
    assign(name, value, envir = env)
    invisible(original)
}

local_unlock_binding <- function(env, name, frame = parent.frame()) {
    if (name %in% names(env) && bindingIsLocked(name, env)) {
        unlockBinding(name, env)
        defer(lockBinding(name, env), envir = frame)
    }
}
