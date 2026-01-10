#
# repos.R
#
# Copyright (C) 2024 Posit Software, PBC. All rights reserved.
#
#

# Applies defaults to the repos option. Does not override any existing
# repositories set in the option.
#
# Arguments:
#   defaults: A named character vector of default CRAN repositories.
apply_repo_defaults <- function(
    defaults = c(CRAN = "https://cran.rstudio.com/")
) {
    repos <- getOption("repos")
    if (is.null(repos) || !is.character(repos)) {
        # There are no repositories set, so apply the defaults directly
        repos <- defaults
    } else {
        if ("CRAN" %in% names(repos) && "CRAN" %in% names(defaults)) {
            # If a CRAN repository is set to @CRAN@ or is marked as having been
            # updated by "the IDE" *and* a default provides a specific URL,
            # override it.
            #
            # This is the only instance in which we replace an already-set
            # repository option with a default.
            if (identical(repos[["CRAN"]], "@CRAN@") || isTRUE(attr(repos, "IDE"))) {
                repos[["CRAN"]] <- defaults[["CRAN"]]

                # Flag this CRAN repository as being set by the IDE. This flag is
                # used by renv.
                attr(repos, "IDE") <- TRUE
            }
        }
        # Set all the names in the defaults that are not already set
        for (name in names(defaults)) {
            if (!(name %in% names(repos))) {
                repos[[name]] <- defaults[[name]]
            }
        }
    }
    options(repos = repos)
}

#' Set the Posit Package Manager repository
#'
#' Sets the Posit Package Manager repository URL for the current session. The
#' URL will be processed to point to a Linux distribution-specific binary URL if
#' applicable.
#'
#' This function only overrides the CRAN repository when Ark has previously set
#' it or when it uses placeholder `"@CRAN@"`.
#'
#' @param url A PPM repository URL. Must be in the form
#'   `"https://host/repo/snapshot"`, e.g.,
#'   `"https://packagemanager.posit.co/cran/latest"`.
#'
#' @return `NULL`, invisibly.
#'
#' @export
.ps.set_ppm_repo <- function(url) {
    # Use Ark's built-in PPM binary URL detection.
    url <- .ps.Call("ps_get_ppm_binary_url", url)
    apply_repo_defaults(c(CRAN = url))
}
