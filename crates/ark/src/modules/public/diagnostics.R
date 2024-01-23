#
# completions.R
#
# Copyright (C) 2022 Posit Software, PBC. All rights reserved.
#
#

.ps.diagnostics.diagnostic <- function(kind = "skip", node = NULL, message = NULL) {
    list(kind, node, message)
}

.ps.diagnostics.custom.library <- function(call, contents, fun = base::library) {
    # TODO: it could be interesting to have a diagnostic
    #       when this will fail on wrong argument, e.g.
    #       library(pkg = ggplot2)
    #               ^^^ : unused argument (pkg)
    matched_call <- match.call(fun, call)

    # here we get a call where arguments are named, e.g.
    # library(package = <x>) where <x> is a list of 2 things:
    # - a 0-based integer position for this argument. This is not
    #   currently used
    # - an external pointer to a treesitter Node, which can be queried
    #   with .ps.treesitter.node.text() and .ps.treesitter.node.kind()
    #
    # We might simplify and only pass around the external pointer if
    # we realize the position isn't useful.

    # identify if character.only was set, so that we can
    # adapt the diagnostic appropriately
    is_character_only <- function(matched_call, contents) {
        character_only <- matched_call[["character.only"]]

        if (is.null(character_only)) {
            FALSE
        } else {
            ptr <- character_only[[2L]]
            text <- .ps.treesitter.node.text(ptr, contents)
            !identical(text, "FALSE")
        }
    }

    # deal with arguments `package` and `help` which use
    # non standard evaluation, e.g. library(ggplot2)
    diagnostic_package <- function(arg, contents, character_only) {
        index <- arg[[1L]]
        node <- arg[[2L]]

        kind <- .ps.treesitter.node.kind(node)

        # library(foo, character.only = TRUE)
        if (kind == "identifier" && character_only) {
            return(.ps.diagnostics.diagnostic("default", node))
        }

        if (kind %in% c("string", "identifier")) {
            # library("foo") or library(foo)
            pkg <- .ps.treesitter.node.text(node, contents)

            if (kind == "string") {
                pkg <- gsub("^(['\"])(.*)\\1$", "\\2", pkg)
            }

            # The package is installed, just skip the diagnostic
            if (pkg %in% base::.packages(all.available = TRUE)) {
                return(.ps.diagnostics.diagnostic("skip"))
            }

            msg <- sprintf("Package '%s' is not installed", pkg)
            return(.ps.diagnostics.diagnostic("simple", node, message = msg))
        }

        .ps.diagnostics.diagnostic("default", node)
    }

    # Before scanning all arguments, we need to check if
    # character.only is set, so that we can adapt how the
    # package and help arguments are handled
    character_only <- is_character_only(matched_call, contents)

    # Scan the given arguments and make diagnostics for each
    n <- length(matched_call)
    out <- vector("list", n - 1L)
    names <- names(matched_call)
    for (i in seq_len(n - 1L)) {
        arg <- matched_call[[i + 1L]]
        name <- names[[i + 1L]]

        diagnostic <- if (name %in% c("package", "help")) {
            diagnostic_package(arg, contents, character_only)
        } else {
            .ps.diagnostics.diagnostic("default", node = arg[[2L]])
        }

        out[[i]] <- diagnostic
    }

    out
}

.ps.diagnostics.custom.require <- function(call, contents, fun = base::require) {
    .ps.diagnostics.custom.library(call, contents, fun)
}
