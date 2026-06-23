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
    ip <- utils::installed.packages(fields = c("Description", "URL"))
    name <- ip[, "Package"]
    version <- ip[, "Version"]
    id <- paste0(name, "-", version)
    # `attached` mirrors search() membership; we deliberately don't use
    # "loaded" here since loadedNamespaces() is a strict superset (a
    # package can be loaded as a dependency without being attached).
    attached <- paste0("package:", name) %in% search()
    # DESCRIPTION wraps Description across lines; R's DCF parser preserves
    # the embedded newlines and runs of spaces. Collapse all whitespace
    # runs to a single space and strip the edges so the card renders as
    # one flowing string. NA (missing field) becomes "".
    description <- trimws(gsub(
        "\\s+",
        " ",
        ifelse(is.na(ip[, "Description"]), "", ip[, "Description"]),
        perl = TRUE
    ))
    # DESCRIPTION's URL field can list several URLs separated by commas and/or
    # whitespace; the first is conventionally the package's canonical website.
    # Surface it as the single best URL (Positron validates the scheme before
    # opening). Drop everything from the first separator on; "" when absent.
    url <- sub("[[:space:],].*$", "", trimws(ip[, "URL"]))
    # Build each package as a list so `url` can be omitted entirely when a
    # package advertises none -- a vectorized `Map(list, url = url)` forces the
    # key onto every package (serializing "" rather than leaving it absent).
    # `Map`'s first argument (`id`, a character vector) would name the result,
    # so `unname()` keeps it a JSON array rather than an object keyed by id.
    unname(Map(
        function(id, name, version, attached, description, url) {
            entry <- list(
                id = id,
                name = name,
                displayName = name,
                version = version,
                attached = attached,
                description = description
            )
            entry$url <- if (!is.na(url)) url
            entry
        },
        id,
        name,
        version,
        attached,
        description,
        url
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

# Return detail fields for a single installed package by name.
#' @export
.ps.rpc.pkg_detail <- function(name) {
    installed <- rownames(utils::installed.packages())
    if (!nzchar(name) || !(name %in% installed)) {
        return(NULL)
    }
    fields <- c(
        "Title",
        "Author",
        "Maintainer",
        "License",
        "Depends",
        "Imports",
        "LinkingTo",
        "Repository",
        "Date/Publication"
    )
    d <- utils::packageDescription(name, fields = fields)

    # Collapse DCF whitespace; return NULL for missing (NA) fields.
    clean <- function(x) {
        if (is.null(x) || is.na(x)) {
            return(NULL)
        }
        trimws(gsub("\\s+", " ", x, perl = TRUE))
    }

    # Parse a comma-separated dependency field into bare package names
    # (strip version constraints like "(>= 1.0)").
    parse_deps <- function(x) {
        if (is.null(x) || is.na(x)) {
            return(character(0))
        }
        parts <- trimws(strsplit(x, ",")[[1]])
        names <- trimws(sub("\\(.*\\)", "", parts))
        names[nzchar(names)]
    }

    base_pkgs <- rownames(utils::installed.packages(priority = "base"))
    deps <- unique(c(
        parse_deps(d$Depends),
        parse_deps(d$Imports),
        parse_deps(d$LinkingTo)
    ))
    deps <- setdiff(deps, c("R", base_pkgs))

    out <- list(name = name, dependencyCount = length(deps))

    # Prefer Maintainer for author display; fall back to Author.
    author <- clean(d$Maintainer)
    if (is.null(author)) {
        author <- clean(d$Author)
    }
    title <- clean(d$Title)
    license <- clean(d$License)
    repo <- clean(d$Repository)
    published <- clean(d[["Date/Publication"]])

    if (!is.null(title)) {
        out$title <- title
    }
    if (!is.null(author)) {
        out$author <- author
    }
    if (!is.null(license)) {
        out$license <- license
    }
    if (!is.null(repo)) {
        out$sourceRepository <- repo
    }
    if (!is.null(published)) {
        out$publishedDate <- published
    }

    out
}


# Return the list of outdated packages with their latest available versions.
#
# `utils::old.packages()` queries the user's configured repositories, so
# `ReposVer` is the authoritative latest version for this session -- it reflects
# what an upgrade would actually fetch, which P3M (a generic upstream mirror)
# cannot guarantee.
#' @export
.ps.rpc.pkg_outdated <- function() {
    outdated <- utils::old.packages()
    if (is.null(outdated) || nrow(outdated) == 0) {
        return(list())
    }
    unname(Map(
        list,
        name = outdated[, "Package"],
        latestVersion = outdated[, "ReposVer"]
    ))
}
