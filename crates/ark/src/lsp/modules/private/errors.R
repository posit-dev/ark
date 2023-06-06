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
        traceback <- traceback[seq_len(n - 3L)]
    }

    # TODO: Can we clean up the base traceback anymore?
    # Maybe we can number it? This would require us to
    # indent multiline calls correctly relative to the
    # leading number.
    traceback <- as.character(traceback)
    
    .ps.Call("ps_record_error", evalue, traceback)
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