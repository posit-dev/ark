#
# completions.R
#
# Copyright (C) 2022 Posit Software, PBC. All rights reserved.
#
#

.ps.diagnostics.diagnostic <- function(kind = "skip", node = NULL, message = NULL) {
    list(kind, node, message)
}

.ps.diagnostics.custom.library <- function(call, source, fun = base::library) {
    matched_call <- match.call(fun, call)

    is_character_only <- function(matched_call, source) {
        character_only <- matched_call[["character.only"]]

        if (is.null(character_only)) {
            FALSE
        } else {
            ptr <- character_only[[2L]]
            text <- .ps.treesitter.node.text(ptr, source)
            !(text %in% c("FALSE", "F"))
        }
    }

    # deal with arguments `package` and `help` which use
    # non standard evaluation, e.g. library(ggplot2)
    diagnostic_package <- function(arg, source, character_only) {
        index <- arg[[1L]]
        node <- arg[[2L]]

        kind <- .ps.treesitter.node.kind(node)

        # library(foo, character.only = TRUE)
        if (kind == "identifier" && character_only) {
            return(.ps.diagnostics.diagnostic("default", node))
        }

        if (kind %in% c("string", "identifier")) {
            # library("foo") or library(foo)
            pkg <- .ps.treesitter.node.text(node, source)

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
    character_only <- is_character_only(matched_call, source)

    # Scan the given arguments and make diagnostics for each
    n <- length(matched_call)
    out <- vector("list", n - 1L)
    names <- names(matched_call)
    for (i in seq_len(n - 1L)) {
        arg <- matched_call[[i + 1L]]
        name <- names[[i + 1L]]

        diagnostic <- if (name %in% c("package", "help")) {
            diagnostic_package(arg, source, character_only)
        } else {
            .ps.diagnostics.diagnostic("skip")
        }

        out[[i]] <- diagnostic
    }

    out
}

.ps.diagnostics.custom.require <- function(call, source, fun = base::require) {
    .ps.diagnostics.custom.library(call, source, fun)
}
