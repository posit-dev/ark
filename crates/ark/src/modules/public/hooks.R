#
# hooks.R
#
# Copyright (C) 2023 Posit Software, PBC. All rights reserved.
#
#

.ps.register_utils_hook <- function(name, hook, namespace = FALSE) {
    package_name <- "package:utils"
    # check if function exists
    if (!exists(name, package_name, mode = "function")) {
        msg <- sprintf("Could not register hook: function `utils::%s` not found", name)
        stop(msg, call. = FALSE)
    }
    # replaces the unnamespaced, attached version of the function
    utils_env <- as.environment(package_name)
    .ps.binding.replace(name, hook, utils_env)

    # if namespace = TRUE, we try to replace the binding for `utils::name` as well
    if (namespace) {
        utils_ns <- asNamespace("utils")
        if (!exists(name, utils_ns, mode = "function")) {
            msg <- sprintf("Could not replace `%s` in the `utils` namespace", name)
            warning(msg, call. = FALSE)
        } else {
            .ps.binding.replace(name, hook, utils_ns)
        }
    }
}

# Wrapper to contain the definition of all hooks we want to register
.ps.register_all_hooks <- function() {
    .ps.register_utils_hook("View", .ps.view_data_frame, namespace = TRUE)
}
