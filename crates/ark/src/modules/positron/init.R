#
# init.R
#
# Copyright (C) 2024 Posit Software, PBC. All rights reserved.
#
#

import_positron <- function(exprs) {
    init_positron()

    # Namespace is created by the sourcer of this file
    ns <- parent.env(environment())
    local_unlock(ns)

    source(exprs = exprs, local = ns)
    export(exprs, from = ns, to = as.environment("tools:positron"))
}

init_positron <- function() {
    # Already initialised if we're on the search path
    if ("tools:positron" %in% search()) {
        return()
    }

    # Create environment for functions exported on the search path
    attach(list(), name = "tools:positron")

    # Lock it, we'll unlock when updating
    lockEnvironment(as.environment("tools:positron"))
}

export <- function(exprs, from, to) {
    local_unlock(to)

    for (name in exported_names(exprs)) {
        to[[name]] <- from[[name]]
    }
}

exported_names <- function(exprs) {
    data <- utils::getParseData(exprs)

    # If `keep.source` was `FALSE`
    if (is.null(data)) {
        return(character())
    }

    exported <- character()
    exported_locs <- which(data$text == "#' @export")

    for (loc in exported_locs) {
        # The data frame is arranged in token order but contains AST nodes
        # too (indicated in the `token` column as `expr`). Skip nodes until
        # next symbol. Should normally be `loc + 2L` but we try not to make
        # assumptions about the AST.
        while (data$token[[loc]] != "SYMBOL") {
            loc <- loc + 1L
        }
        exported <- c(exported, data$text[[loc]])
    }

    exported
}

import_rstudio <- function(exprs) {
    init_rstudio()

    env <- rstudio_ns()
    local_unlock(env)

    source(exprs = exprs, local = env)
    export(exprs, from = env, to = as.environment("tools:rstudio"))
}

init_rstudio <- function() {
    # Already initialised if we're on the search path
    if ("tools:rstudio" %in% search()) {
        return()
    }

    # Create environment for the rstudio namespace.
    # It inherits from the positron namespace.
    parent <- parent.env(environment())
    the$rstudio_ns <- new.env(parent = parent)

    # Create environment for functions exported on the search path
    attach(list(), name = "tools:rstudio")

    # Lock environments, we'll unlock them before updating
    lockEnvironment(the$rstudio_ns)
    lockEnvironment(as.environment("tools:rstudio"))

    # Override `rstudioapi::isAvailable()` so it thinks it's running under RStudio
    setHook(
        packageEvent("rstudioapi", "onLoad"),
        function(...) {
            ns <- asNamespace("rstudioapi")

            unlockBinding("isAvailable", ns)
            on.exit(lockBinding("isAvailable", ns))

            # TODO: Should check for `version_needed`
            body(ns$isAvailable) <- TRUE
        }
    )
}

rstudio_ns <- function() {
    the$rstudio_ns
}


# Tools used in this file. Must stay here to be self-contained.

.ps.Call <- function(.NAME, ...) {
    .Call(.NAME, ..., PACKAGE = "(embedding)")
}

env_unlock <- function(env) {
    .ps.Call("ark_env_unlock", env)
}

defer <- function(expr, envir = parent.frame(), after = FALSE) {
  thunk <- as.call(list(function() expr))
  do.call(
    on.exit,
    list(thunk, add = TRUE, after = after),
    envir = envir
  )
}

local_unlock <- function(env, frame = parent.frame()) {
    if (environmentIsLocked(env)) {
        env_unlock(env)
        defer(lockEnvironment(env), envir = frame)
    }
}
