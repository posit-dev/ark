#
# positron.R
#
# Copyright (C) 2025 Posit Software, PBC. All rights reserved.
#
#

if (Sys.getenv("POSITRON") == 1) {
    .Platform <- base::.Platform
    .Platform$GUI <- "Positron"
    env_bind_force(baseenv(), ".Platform", .Platform)
}
