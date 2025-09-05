#
# variables.R
#
# Copyright (C) 2025 Posit Software, PBC. All rights reserved.
#
#

setHook(
    packageEvent("survival", "onLoad"),
    function(...) {
        # Register special methods for Surv objects.
        # Surv objects are matrices, but unlike matrices indexing using [
        # will index a row, and not an element of the matrix which is assumed
        # by the variables pane formatting code.
        .ark.register_method(
            "ark_positron_variable_get_children",
            "Surv",
            function(x, width) {
                list(
                    time = x[, 1],
                    status = x[, 2]
                )
            }
        )

        .ark.register_method(
            "ark_positron_variable_get_child_at",
            "Surv",
            function(x, ..., index, name) {
                if (!is.null(name)) {
                    x <- x[, name]
                } else {
                    x <- x[, index]
                }
            }
        )
    }
)
