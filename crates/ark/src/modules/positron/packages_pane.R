#
# packages_pane.R
#
# Copyright (C) 2026 Posit Software, PBC. All rights reserved.
#
#

# This file contains RPC functions for the packages pane.
# These functions are called via callMethod from the Positron R extension.

# Return a list of installed packages.
#
# The `method` parameter is accepted for compatibility with the existing
# Positron R extension RPC contract but is unused: base R's
# installed.packages() reads from .libPaths(), which renv overrides in an
# active project, so the same call works for pak/base/renv callers.
#' @export
.ps.rpc.pkg_list <- function(method = c("pak", "base", "renv")) {
    ip <- utils::installed.packages()
    name <- ip[, "Package"]
    version <- ip[, "Version"]
    id <- paste0(name, "-", version)
    # `attached` mirrors search() membership; we deliberately don't use
    # "loaded" here since loadedNamespaces() is a strict superset (a
    # package can be loaded as a dependency without being attached).
    attached <- paste0("package:", name) %in% search()
    # `Map(list, id = ...)` would name the output list by `id`'s values
    # (mapply USE.NAMES semantics for a character first arg), and a named
    # list serializes to a JSON object instead of the array the frontend
    # expects. Strip the names with unname().
    unname(Map(
        list,
        id = id,
        name = name,
        displayName = name,
        version = version,
        attached = attached
    ))
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
