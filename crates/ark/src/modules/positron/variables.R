#
# variables.R
#
# Copyright (C) 2025 Posit Software, PBC. All rights reserved.
#
#

.ark.register_method(
    "ark_positron_variable_display_value",
    "Surv",
    function(x, width) {
        paste(base::format(x), collapse = " ")
    }
)

.ark.register_method(
    "ark_positron_variable_display_type",
    "Surv",
    function(x, include_length) {
        sprintf("%s [%d]", "Surv", length(x))
    }
)

.ark.register_method("ark_positron_variable_kind", "Surv", function(x) {
    "other"
})
