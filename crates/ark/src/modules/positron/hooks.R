#
# hooks.R
#
# Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
#
#

#' @export
.ps.register_utils_hook <- function(name, hook, namespace = FALSE) {
  if (namespace) {
    hook_namespace <- hook
  } else {
    hook_namespace <- NULL
  }

  pkg_hook(
    pkg = "utils",
    name = name,
    hook = hook,
    hook_namespace = hook_namespace
  )
}

# Wrapper to contain the definition of all hooks we want to register
#' @export
.ps.register_all_hooks <- function() {
  .ps.register_utils_hook("View", .ps.view_data_frame, namespace = TRUE)
  register_onload_hook()
}

#' Override a function within an attached package
#'
#' Assumes the package is attached, typically used for base packages like base or utils.
#' - `hook` will replace the binding for unnamespaced calls.
#' - `hook_namespace` will optionally also replace the binding for namespaced calls.
#'
#' TODO: Will cause ark to fail to start if `option(defaultPackages = character())`
#' or `R_DEFAULT_PACKAGES=NULL` are set! One idea is to register an `onAttach()`
#' hook here and use that if the package is not loaded yet.
pkg_hook <- function(pkg, name, hook, hook_namespace = NULL) {
  env <- sprintf("package:%s", pkg)
  env <- as.environment(env)

  if (!exists(name, envir = env, mode = "function", inherits = FALSE)) {
    msg <- sprintf("Can't register hook: function `%s::%s` not found.", pkg, name)
    stop(msg, call. = FALSE)
  }

  # Replaces the unnamespaced, attached version of the function
  hook_original <- env_bind_force(env, name, hook)

  # If `hook_namespace` is provided, we try to replace the binding for `pkg::name` as well
  if (is.null(hook_namespace)) {
    hook_namespace_original <- NULL
  } else {
    ns <- asNamespace(pkg)
    if (!exists(name, envir = ns, mode = "function", inherits = FALSE)) {
      msg <- sprintf("Can't replace `%s` in the '%s' namespace.", name, pkg)
      stop(msg, call. = FALSE)
    }
    hook_namespace_original <- env_bind_force(ns, name, hook_namespace)
  }

  invisible(list(
    hook = hook_original,
    hook_namespace = hook_namespace_original
  ))
}

# R only allows `onLoad` hooks for named packages, not for any package that
# might be loaded in the session. We modify `getHook()` to add support for
# such a general event.
register_onload_hook <- function() {
    ns <- asNamespace("base")
    local_unlock_binding(ns, "getHook")

    ns[["getHook"]] <- function(hookName, ...) {
        hooks <- get0(hookName, envir = .userHooksEnv, inherits = FALSE, ifnotfound = list())

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
    .ps.Call("ps_onload_hook", pkg, path)
}
ark_onload_hook <- structure(
    ark_onload_hook,
    class = c("ark_onload_hook", "function")
)
