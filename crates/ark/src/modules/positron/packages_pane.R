#
# packages_pane.R
#
# Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
#
#

# This file contains functions related to the packages pane.
# pkg_list, pkg_search, and pkg_search_versions are called for a result using `callMethod`.
# pkg_install, pkg_update_all, and pkg_uninstall are executed interactively in the console.

#' @export
.ps.rpc.pkg_list <- function(method = c("pak", "base")) {
    method <- match.arg(method)
    switch(
        method,
        pak = {
            old_opt <- options(pak.no_extra_messages = TRUE)
            on.exit(options(old_opt), add = TRUE)
            pkgs <- pak::lib_status()
            lapply(seq_len(nrow(pkgs)), function(i) {
                list(
                    id = paste0(pkgs$package[[i]], "-", pkgs$version[[i]]),
                    name = pkgs$package[[i]],
                    displayName = pkgs$package[[i]],
                    version = as.character(pkgs$version[[i]])
                )
            })
        },
        base = {
            ip <- utils::installed.packages()
            lapply(seq_len(nrow(ip)), function(i) {
                list(
                    id = paste0(ip[i, "Package"], "-", ip[i, "Version"]),
                    name = ip[i, "Package"],
                    displayName = ip[i, "Package"],
                    version = ip[i, "Version"]
                )
            })
        }
    )
}


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

#' @export
.ps.rpc.pkg_search_versions <- function(name) {
    ap <- utils::available.packages()
    version <- if (name %in% rownames(ap)) ap[name, "Version"] else character(0)
    # Wrap in as.list() to ensure it serializes as an array, not a scalar
    as.list(version)
}

#' @export
.ps.rpc.pkg_outdated <- function() {
    outdated <- utils::old.packages()
    if (is.null(outdated) || nrow(outdated) == 0) {
        return(list())
    }
    # Return as list to ensure it serializes as an array
    as.list(outdated[, "Package"])
}


#' @export
.ps.rpc.pkg_install <- function(packages, method = c("pak", "base")) {
    # Convert from list to character vector (JSON arrays arrive as lists)
    packages <- unlist(packages)
    method <- match.arg(method)
    switch(
        method,
        pak = pak::pkg_install(packages, ask = FALSE),
        base = utils::install.packages(packages)
    )

    # Return a value, void doesn't serialize
    TRUE
}

#' @export
.ps.rpc.pkg_uninstall <- function(packages, method = c("pak", "base")) {
    # Convert from list to character vector (JSON arrays arrive as lists)
    packages <- unlist(packages)
    method <- match.arg(method)
    switch(
        method,
        pak = pak::pkg_remove(packages),
        base = utils::remove.packages(packages)
    )
    for (pkg in packages) {
        try(unloadNamespace(pkg), silent = TRUE)
    }

    # Return a value, void doesn't serialize
    TRUE
}

#' @export
.ps.rpc.pkg_update_all <- function(method = c("pak", "base")) {
    method <- match.arg(method)
    switch(
        method,
        pak = {
            old_opt <- options(pak.no_extra_messages = TRUE)
            on.exit(options(old_opt), add = TRUE)
            outdated <- utils::old.packages()[, "Package"]
            if (length(outdated) > 0) {
                pak::pkg_install(outdated, ask = FALSE)
            }
        },
        base = utils::update.packages(ask = FALSE)
    )

    # Return a value, void doesn't serialize
    TRUE
}
