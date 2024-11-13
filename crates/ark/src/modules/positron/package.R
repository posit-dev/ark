#
# package.R
#
# Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
#
#

# Checks if a package is installed without loading it.
# Could be slow on network drives.
#' @export
.ps.is_installed <- function(pkg, minimum_version = NULL) {
    installed <- system.file(package = pkg) != ""

    if (installed && !is.null(minimum_version)) {
        installed <- utils::packageVersion(pkg) >= minimum_version
    }

    installed
}

#' @export
.ps.rpc.is_installed <- .ps.is_installed

# Returns a list containing:
#   * the version string if the package is installed and NULL otherwise
#   * a minimum version to check for, with '0.0.0' as a dummy default
#   * a logical indicating if package is installed at or above the minimum version
#  This may seem weird, but it's impractical for positron-r to do version
#  comparisons.
#' @export
.ps.rpc.packageVersion <- function(pkg, checkAgainst = "0.0.0") {
    installed <- system.file(package = pkg) != ""

    if (installed) {
        version <- utils::packageVersion(pkg)
        list(
          version = as.character(version),
          checkAgainst = as.character(checkAgainst),
          checkResult = version >= checkAgainst
        )
    } else {
        list(
          version = NULL,
          checkAgainst = as.character(checkAgainst),
          checkResult = FALSE
        )
    }
}

#' @export
.ps.rpc.install_packages <- function(packages) {
    for (pkg in packages) {
        if (.ps.rpc.isPackageAttached(pkg)) {
            stop("Should not install a package if it's already attached.")
        }
    }
    utils::install.packages(packages)
    TRUE
}

#' @export
.ps.rpc.isPackageAttached <- function(pkg) {
    if (!is_string(pkg)) {
        stop("`pkg` must be a string.")
    }

    pkg %in% .packages()
}
