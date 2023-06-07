#
# errors.R
#
# Copyright (C) 2023 Posit Software, PBC. All rights reserved.
#
#

handle_error_base <- function(cnd) {
    # Rough equivalent of `rlang::cnd_message(prefix = TRUE)`
    evalue <- conditionMessage(cnd)
    evalue <- paste0("Error:\n", evalue)

    traceback <- sys.calls()

    # Converts pairlist to list, and rare `NULL` result to `list()`
    traceback <- as.list(traceback)

    n <- length(traceback)
    if (n >= 3L) {
        # Remove last three frames:
        # - 1 for `handle_error_base()`
        # - 2 for global handler frames
        n <- n - 3L
        traceback <- traceback[seq_len(n)]
    }

    # TODO: This implementation prints the traceback in the same ordering
    # as rlang, i.e. with call 1 on the stack being the first thing you
    # see. `traceback()` prints in the reverse order, so users may want a
    # way to reverse both our display ordering and rlang's (requiring a
    # new `rlang::format()` argument).

    # Converts to a list of character vectors containing the deparsed calls.
    # Respects global options `"traceback.max.lines"` and `"deparse.max.lines"`!
    traceback <- .traceback(traceback)

    # Prepend the stack number to each deparsed call, padding multiline calls as needed,
    # and then collapse multiline calls into one line
    prefixes <- vec_paste0(seq_len(n), ". ")
    prefixes <- format(prefixes, justify = "right")

    traceback <- mapply(prepend_prefix, traceback, prefixes, SIMPLIFY = FALSE)
    traceback <- lapply(traceback, function(lines) paste0(lines, collapse = "\n"))
    traceback <- as.character(traceback)
    
    .ps.Call("ps_record_error", evalue, traceback)
}

prepend_prefix <- function(lines, prefix) {
    n_lines <- length(lines)

    if (n_lines == 0L) {
        return(lines)
    }

    # First line gets the prefix
    line <- lines[[1L]]
    line <- vec_paste0(prefix, line)

    # Other lines are padded with whitespace as needed
    padding <- strrep(" ", times = nchar(prefix))

    lines <- lines[-1L]
    lines <- vec_paste0(padding, lines)

    lines <- c(line, lines)

    lines
}

handle_error_rlang <- function(cnd) {
    evalue <- rlang::cnd_message(cnd, prefix = TRUE)
    
    if (is.null(cnd$trace)) {
        traceback <- character()
    } else if (rlang::trace_length(cnd$trace) == 0L) {
        # Avoid showing traceback tree node when the trace is empty
        traceback <- character()
    } else {
        # Calls rlang specific `format()` method for the traceback
        traceback <- format(cnd$trace)
    }

    .ps.Call("ps_record_error", evalue, traceback)
}

positron_option_error_entrace <- function() {
    # TODO: Wire this up to a Positron option for easy toggling?
    isTRUE(getOption("positron.error_entrace", default = TRUE))
}