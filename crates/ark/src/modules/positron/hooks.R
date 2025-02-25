#
# hooks.R
#
# Copyright (C) 2023-2025 Posit Software, PBC. All rights reserved.
#
#

register_hooks <- function() {
    rebind("utils", "View", .ps.view_data_frame, namespace = TRUE)
    rebind("base", "debug", new_ark_debug(base::debug), namespace = TRUE)
    rebind(
        "base",
        "debugonce",
        new_ark_debug(base::debugonce),
        namespace = TRUE
    )
    register_getHook_hook()
}

rebind <- function(pkg, name, value, namespace = FALSE) {
    pkg_bind(
        pkg = pkg,
        name = name,
        value = value
    )

    if (namespace) {
        ns_bind(
            pkg = pkg,
            name = name,
            value = value
        )
    }
}

#' Override a function within an attached package
#'
#' Assumes the package is attached, typically used for base packages like base or utils.
#' - `pkg_bind()` replaces the binding in the package environment on the search
#'   path for unnamespaced calls.
#' - `ns_bind()` replaces the binding for namespaced calls.
#'
#' TODO: Will cause ark to fail to start if `option(defaultPackages = character())`
#' or `R_DEFAULT_PACKAGES=NULL` are set! One idea is to register an `onAttach()`
#' hook here and use that if the package is not loaded yet.
pkg_bind <- function(pkg, name, value) {
    env <- sprintf("package:%s", pkg)
    env <- as.environment(env)

    if (!exists(name, envir = env, mode = "function", inherits = FALSE)) {
        msg <- sprintf(
            "Can't register hook: function `%s::%s` not found.",
            pkg,
            name
        )
        stop(msg, call. = FALSE)
    }

    old <- env_bind_force(env, name, value)
    invisible(old)
}

ns_bind <- function(pkg, name, value) {
    ns <- asNamespace(pkg)

    if (!exists(name, envir = ns, mode = "function", inherits = FALSE)) {
        msg <- sprintf("Can't replace `%s` in the '%s' namespace.", name, pkg)
        stop(msg, call. = FALSE)
    }

    old <- env_bind_force(ns, name, value)
    invisible(old)
}

# R only allows `onLoad` hooks for named packages, not for any package that
# might be loaded in the session. We modify `getHook()` to add support for
# such a general event.
register_getHook_hook <- function() {
    ns <- asNamespace("base")
    local_unlock_binding(ns, "getHook")

    ns[["getHook"]] <- function(hookName, ...) {
        hooks <- get0(
            hookName,
            envir = .userHooksEnv,
            inherits = FALSE,
            ifnotfound = list()
        )

        if (!grepl("^UserHook::.*::onLoad$", hookName)) {
            return(hooks)
        }

        is_ark_hook <- function(fn) {
            inherits(fn, "ark_onload_hook")
        }

        # Inject our onload hook but only if not already there
        if (is.na(Position(is_ark_hook, hooks))) {
            c(list(ark_onload_hook), hooks)
        } else {
            hooks
        }
    }
}

ark_onload_hook <- function(pkg, path) {
    # For compatibility with older pkgload versions
    # https://github.com/r-lib/pkgload/commit/b4e178bd52182a2d7f650754830c69fe51be4b8b
    if (missing(path)) {
        path <- NULL
    }

    # Just in case
    if (!is_string(pkg) || !is_string(path)) {
        return()
    }

    if (pkg == "cli") {
        the$cli_version <- utils::packageVersion("cli")
    }

    check_version(pkg)

    .ps.Call("ps_onload_hook", pkg, path)
}

ark_onload_hook <- structure(
    ark_onload_hook,
    class = c("ark_onload_hook", "function")
)

check_version <- function(pkg) {
    version <- utils::packageVersion(pkg)

    switch(
        pkg,
        roxygen2 = if (version <= "7.3.1") {
            message(paste(
                sep = "\n",
                sprintf(
                    "%s You are using roxygen2 %s. It is recommended to update to roxygen2 >= 7.3.2 when running inside Ark.",
                    ansi_info(),
                    version
                ),
                sprintf(
                    "%s See https://github.com/posit-dev/positron/issues/3053",
                    ansi_info()
                )
            ))
        }
    )
}
