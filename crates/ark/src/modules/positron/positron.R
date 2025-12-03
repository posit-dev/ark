#
# positron.R
#
# Copyright (C) 2025 Posit Software, PBC. All rights reserved.
#
#

.Platform <- base::.Platform
.Platform$GUI <- "Positron"
if (Sys.getenv("POSITRON") == 1) {
    env_bind_force(baseenv(), ".Platform", .Platform)
}
