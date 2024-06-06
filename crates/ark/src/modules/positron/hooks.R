#
# hooks.R
#
# Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
#
#

#' @export
.ps.register_utils_hook <- function(name, hook, namespace = FALSE) {
    package_name <- "package:utils"
    # check if function exists
    if (!exists(name, package_name, mode = "function")) {
        msg <- sprintf("Could not register hook: function `utils::%s` not found", name)
        stop(msg, call. = FALSE)
    }
    # replaces the unnamespaced, attached version of the function
    utils_env <- as.environment(package_name)
    env_bind_force(utils_env, name, hook)

    # if namespace = TRUE, we try to replace the binding for `utils::name` as well
    if (namespace) {
        utils_ns <- asNamespace("utils")
        if (!exists(name, utils_ns, mode = "function")) {
            msg <- sprintf("Could not replace `%s` in the `utils` namespace", name)
            warning(msg, call. = FALSE)
        } else {
            env_bind_force(utils_ns, name, hook)
        }
    }
}

# TODO: Merge https://github.com/posit-dev/amalthea/pull/383 first
# and use the utilities there to make this cleaner
#' @export
.ps.register_base_hook <- function(name, hook, namespace = FALSE) {
    package_name <- "package:base"
    # check if function exists
    if (!exists(name, package_name, mode = "function")) {
        msg <- sprintf("Could not register hook: function `base::%s` not found", name)
        stop(msg, call. = FALSE)
    }
    # replaces the unnamespaced, attached version of the function
    utils_env <- as.environment(package_name)
    env_bind_force(utils_env, name, hook)

    # if namespace = TRUE, we try to replace the binding for `base::name` as well
    if (namespace) {
        utils_ns <- asNamespace("base")
        if (!exists(name, utils_ns, mode = "function")) {
            msg <- sprintf("Could not replace `%s` in the `base` namespace", name)
            warning(msg, call. = FALSE)
        } else {
            env_bind_force(utils_ns, name, hook)
        }
    }
}

# `try()` is the one other place in base R besides `stop()` that utilizes
# `show.error.messages`. We forcibly set `show.error.messages = FALSE` when
# initializing our global calling handler for errors, which causes `try()` to
# always act like `silent = TRUE` is set. Unlike errors that go through `stop()`,
# `try()` does not offer any other way for us to capture the error output and
# relay it to the user ourselves (like through global calling handlers). Our
# best solution for this is to override `try()` to locally force
# `show.error.messages` back to `TRUE`. This prevents the user from tweaking
# this option, but we don't expect them to do that anyways.
.ps.register_try_hook <- function() {
    fn <- base::try

    # Can't use `local_options()` in the body, must be base code
    body <- body(fn)
    body <- bquote({
        old <- options(show.error.messages = TRUE)
        on.exit(options(old), add = TRUE, after = FALSE)
        .(body)
    })
    body(fn) <- body

    .ps.register_base_hook("try", fn, namespace = TRUE)
}

# Wrapper to contain the definition of all hooks we want to register
#' @export
.ps.register_all_hooks <- function() {
    .ps.register_utils_hook("View", .ps.view_data_frame, namespace = TRUE)
    .ps.register_try_hook()
}
