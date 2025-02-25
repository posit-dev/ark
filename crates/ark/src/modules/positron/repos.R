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
            # If a CRAN repository is set to @CRAN@, and a default provides a
            # specific URL, override it. This is the only instance in which we
            # replace an already-set repository option with a default.
            if (identical(repos[["CRAN"]], "@CRAN@")) {
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
