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

# Adds `injected` on top of a function body, and update the environment
# (possibly locked) with the new definition
push_body <- function(env, fn_name, injected) {
  fn <- env[[fn_name]]
  body <- body(fn)

  if (is_call(body, "{")) {
    exprs <- c(list(injected), body[-1])
  } else {
    exprs <- list(injected, body)
  }
  body(fn) <- as.call(c(list(quote(`{`)), exprs))

  local_unlock_binding(env, fn_name)
  env[[fn_name]] <- fn

  invisible(NULL)
}

is_call <- function(x, fn) {
  typeof(body) == "language" && identical(body[[1]], as.symbol(fn))
}
