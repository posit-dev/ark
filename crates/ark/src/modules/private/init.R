#
# init.R
#
# Copyright (C) 2024 Posit Software, PBC. All rights reserved.
#
#

import_positron <- function(path) {
    init_positron()

    # Namespace is created by the sourcer of this file
    ns <- parent.env(environment())
    source(path, local = ns)

    export(path, from = ns, to = as.environment("tools:positron"))
}

init_positron <- function() {
    # Already initialised if we're on the search path
    if ("tools:positron" %in% search()) {
        return()
    }

    # Create environment for functions exported on the search path
    attach(list(), name = "tools:positron")
}

export <- function(path, from, to) {
    for (name in exported_names(path)) {
        to[[name]] <- from[[name]]
    }
}

exported_names <- function(path) {
    ast <- parse(path, keep.source = TRUE)
    data <- utils::getParseData(ast)

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

import_rstudioapi_shims <- function(path) {
    init_rstudioapi()

    env <- rstudioapi_shims_env()
    source(path, local = env)

    export(path, from = env, to = as.environment("tools:rstudio"))
}

init_rstudioapi <- function() {
    # Already initialised if we're on the search path
    if ("tools:rstudio" %in% search()) {
        return()
    }

    # Create environment for the rstudioapi shims.
    # It inherits from Ark's namespace.
    parent <- parent.env(environment())
    the$rstudioapi_shims_env <- new.env(parent = parent)

    # Create environment for functions exported on the search path
    attach(list(), name = "tools:rstudio")

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

rstudioapi_shims_env <- function() {
    the$rstudioapi_shims_env
}
