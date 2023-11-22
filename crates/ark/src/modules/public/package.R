#
# package.R
#
# Copyright (C) 2023 Posit Software, PBC. All rights reserved.
#
#

# Checks if a package is installed without loading it.
# Could be slow on network drives.
.ps.is_installed <- function(pkg, minimum_version = NULL) {
    installed <- system.file(package = pkg) != ""

    if (installed && !is.null(minimum_version)) {
        installed <- packageVersion(pkg) >= minimum_version
    }

    installed
}
