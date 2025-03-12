#
# renv.R
#
# Copyright (C) 2025 Posit Software, PBC. All rights reserved.
#
#

is_renv_1_0_1_or_earlier <- function() {
    tryCatch(
        {
            utils::packageVersion("renv") <= "1.0.1"
        },
        error = function(e) FALSE
    )
}
