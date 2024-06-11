#
# errors.R
#
# Copyright (C) 2023 Posit Software, PBC. All rights reserved.
#
#

#' @export
.ps.errors.initializeGlobalErrorHandler <- function() {
    if (getRversion() < "4.0.0") {
        # `globalCallingHandlers()` didn't exist here.
        # In this case, we simply print errors to the console.
        # We will never throw an `ExecuteReplyException` here.
        return(invisible(NULL))
    }

    # Unregister all handlers and hold onto them
    handlers <- globalCallingHandlers(NULL)

    # Inject our global error handler at the end.
    # This allows other existing error handlers to run ahead of us.
    handlers <- c(
        handlers,
        list(
            error = .ps.errors.globalErrorHandler,
            message = .ps.errors.globalMessageHandler
        )
    )
    do.call(globalCallingHandlers, handlers)

    invisible(NULL)
}

#' @export
.ps.errors.globalErrorHandler <- function(cnd) {
    if (!.ps.is_installed("rlang")) {
        # rlang is not installed, no option except to use the base handler
        return(handle_error_base(cnd))
    }

    if (!inherits(cnd, "rlang_error") && !positron_option_error_entrace()) {
        # We have a non-rlang error, but the user requested we dont entrace it
        return(handle_error_base(cnd))
    }

    if (!inherits(cnd, "rlang_error")) {
        base_cnd <- cnd
        cnd <- rlang::catch_cnd(rlang::entrace(cnd))

        # rlang might decide not to entrace, e.g. when `recover` is set as
        # global error handler
        if (is.null(cnd)) {
            return(handle_error_base(base_cnd))
        }
    }

    handle_error_rlang(cnd)
}

#' @export
.ps.errors.globalMessageHandler <- function(cnd) {
    # Decline to handle if we can't muffle the message (should only happen
    # in extremely rare cases)
    if (is.null(findRestart("muffleMessage"))) {
        return()
    }

    msg <- conditionMessage(cnd)

    if (inherits(cnd, "rlang_message")) {
        # Special-case for rlang messages which use the implicit trailing
        # line feed approach of warnings and errors. See
        # https://github.com/posit-dev/positron/issues/1878 for context and
        # https://github.com/r-lib/rlang/issues/1677 for a discussion about
        # making rlang messages consistent with base messages rather than
        # warnings and errors.
        msg <- paste0(msg, "\n")
    }

    # Output the condition message to the relevant stream (normally
    # stdout). Note that for historical reasons, messages include a
    # trailing newline
    cat(msg, file = default_message_file())

    # Silence default message handling
    invokeRestart("muffleMessage")
}

#' @export
.ps.errors.traceback <- function() {
    traceback <- get0(".Traceback", baseenv(), ifnotfound = list())

    # Be defensive against potential `NULL` as this comes from foreign code
    if (!length(traceback)) {
        return(character())
    }

    format_traceback(traceback)
}

# If a sink is active (either on output or on messages) messages
# are always streamed to `stderr`. This follows rlang behaviour
# and ensures messages can be sinked from stderr consistently.
#
# Unlike rlang we don't make an exception for non-interactive sessions
# since Ark is meant to be run interactively.
default_message_file <- function() {
  if (sink.number("output") == 0 &&
      sink.number("message") == 2) {
    stdout()
  } else {
    stderr()
  }
}

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
    traceback <- format_traceback(traceback)

    .ps.Call("ps_record_error", evalue, traceback)

    # Prevent further error handling, including by R's internal handler that
    # displays the error message
    invokeRestart("abort")
}

#' @param traceback A list of calls.
format_traceback <- function(calls = list()) {
    # Calls the function of the same name in the harp namespace
    .ps.Call("ps_format_traceback", calls)
}

handle_error_rlang <- function(cnd) {
    evalue <- rlang::cnd_message(cnd, prefix = TRUE)
    traceback <- cnd$trace

    if (is.null(traceback)) {
        traceback <- character()
    } else if (rlang::trace_length(traceback) == 0L) {
        # Avoid showing traceback tree node when the trace is empty
        traceback <- character()
    } else {
        # Calls rlang specific `format()` method for the traceback
        traceback <- format(traceback)
    }

    .ps.Call("ps_record_error", evalue, traceback)

    # Prevent further error handling, including by R's internal handler that
    # displays the error message
    invokeRestart("abort")
}

positron_option_error_entrace <- function() {
    # TODO: Wire this up to a Positron option for easy toggling?
    isTRUE(getOption("positron.error_entrace", default = TRUE))
}

rust_backtrace <- function() {
    .ps.Call("ps_rust_backtrace")
}
