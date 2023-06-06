#
# completions.R
#
# Copyright (C) 2022 Posit Software, PBC. All rights reserved.
#
#

.ps.diagnostics.custom <- function(pkg, name, call, context) {
    fun <- get(name, asNamespace(pkg))
    matched_call <- match.call(fun, call)

    if (name %in% c("library", "require")) {
        character_only <- matched_call[["character.only"]]
        character_only <- if (is.null(character_only)) {
            FALSE
        } else {
            ptr <- character_only[[2L]]
            text <- .ps.diagnostics.treesitter.text(ptr, context)
            !(text %in% c("FALSE", "F"))
        }

        n <- length(matched_call)
        out <- vector("list", n - 1L)
        names <- names(matched_call)
        for (i in seq_len(n - 1L)) {
            arg <- matched_call[[i + 1L]]
            name <- names[[i + 1L]]


            if (name %in% c("package", "help")) {
                node <- arg[[2]]
                kind <- .ps.diagnostics.treesitter.kind(node)

                text <- .ps.diagnostics.treesitter.text(node, context)

                if (kind == "string") {
                    pkg <- gsub("^(['\"])(.*)\\1$", "\\2", text)
                    if (pkg %in% base::.packages(all.available = TRUE)) {
                        arg[[3L]] <- "skip"
                    } else {
                        arg[[3L]] <- "simple"
                        arg[[4L]] <- sprintf("Package '%s' is not installed", pkg)
                    }
                } else if (kind == "identifier") {
                    if (character_only) {
                        arg[[3L]] <- "default"
                    } else {
                        pkg <- text
                        if (pkg %in% base::.packages(all.available = TRUE)) {
                            arg[[3L]] <- "skip"
                        } else {
                            arg[[3L]] <- "simple"
                            arg[[4L]] <- sprintf("Package '%s' is not installed", pkg)
                        }
                    }
                } else {
                    arg[[3L]] <- "default"
                }

            } else {
                arg[[3L]] <- "default"
            }

            out[[i]] <- arg
        }

        out
    }
}

.ps.diagnostics.treesitter.text <- function(node, context) {
    .ps.Call("ps_diagnostics_treesitter_text", node, context)
}

.ps.diagnostics.treesitter.kind <- function(node) {
    .ps.Call("ps_diagnostics_treesitter_kind", node)
}
