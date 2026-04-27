#
# packages_pane.R
#
# Copyright (C) 2026 Posit Software, PBC. All rights reserved.
#
#

# This file contains RPC functions for the packages pane.
# These functions are called via callMethod from the Positron R extension.

.ps.pkg_list_installed <- function(lib.loc = NULL) {
    ip <- utils::installed.packages(
        lib.loc = lib.loc,
        fields = c("Description", "Maintainer")
    )

    name <- ip[, "Package"]
    version <- ip[, "Version"]
    id <- paste0(name, "-", version)
    # Collapse whitespace so a multi-line Description fits on one line in the UI.
    description <- trimws(gsub(
        "\\s+",
        " ",
        ifelse(is.na(ip[, "Description"]), "", ip[, "Description"]),
        perl = TRUE
    ))
    # Strip "<email>" markers so "Hadley Wickham <h@posit.co>" becomes "Hadley Wickham".
    author <- trimws(gsub(
        "\\s*<[^>]+>",
        "",
        gsub(
            "\\s+",
            " ",
            ifelse(is.na(ip[, "Maintainer"]), "", ip[, "Maintainer"]),
            perl = TRUE
        ),
        perl = TRUE
    ))

    unname(Map(
        list,
        id = id,
        name = name,
        displayName = name,
        version = version,
        description = description,
        author = author
    ))
}

# Return a list of installed packages. The pak/base/renv methods exist for
# parity with install/update operations; for listing we always use
# utils::installed.packages(), scoped to the renv library when requested.
#' @export
.ps.rpc.pkg_list <- function(method = c("pak", "base", "renv")) {
    method <- match.arg(method)
    lib.loc <- if (method == "renv") renv::paths$library() else NULL
    .ps.pkg_list_installed(lib.loc = lib.loc)
}


# Search the package repository using the given `query` and return a list of matching packages.
#' @export
.ps.rpc.pkg_search <- function(query, method = c("pak", "base")) {
    method <- match.arg(method)
    switch(
        method,
        pak = {
            old_opt <- options(pak.no_extra_messages = TRUE)
            on.exit(options(old_opt), add = TRUE)
            pkgs <- pak::pkg_search(query, size = 100)
            lapply(seq_len(nrow(pkgs)), function(i) {
                list(
                    id = pkgs$package[[i]],
                    name = pkgs$package[[i]],
                    displayName = pkgs$package[[i]],
                    version = "0"
                )
            })
        },
        base = {
            query <- tolower(query)
            ap <- utils::available.packages()
            matches <- ap[
                grepl(query, tolower(ap[, "Package"]), fixed = TRUE),
                ,
                drop = FALSE
            ]
            lapply(seq_len(nrow(matches)), function(i) {
                list(
                    id = matches[i, "Package"],
                    name = matches[i, "Package"],
                    displayName = matches[i, "Package"],
                    version = "0"
                )
            })
        }
    )
}

# Search the package repository for the given package name and return its version if found.
#' @export
.ps.rpc.pkg_search_versions <- function(name) {
    ap <- utils::available.packages()
    version <- if (name %in% rownames(ap)) ap[name, "Version"] else character(0)
    # Wrap in as.list() to ensure it serializes as an array, not a scalar
    as.list(version)
}

# Return the list of outdated pacakages.
#' @export
.ps.rpc.pkg_outdated <- function() {
    outdated <- utils::old.packages()
    if (is.null(outdated) || nrow(outdated) == 0) {
        return(list())
    }
    # Return as list to ensure it serializes as an array
    as.list(outdated[, "Package"])
}
