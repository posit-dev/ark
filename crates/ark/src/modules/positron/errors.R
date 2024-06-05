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

    # Tell rlang and base R not to print the error message, we will do it!
    options(show.error.messages = FALSE)

    invisible(NULL)
}

#' @export
.ps.errors.globalErrorHandler <- function(cnd) {
    # Don't instrument errors if the option has been switched back on
    if (isTRUE(getOption("show.error.messages", TRUE))) {
        return()
    }

    # Decline to handle if a `utils::recover` handler is installed
    if (has_recover_handler()) {
        return()
    }

    if (!.ps.is_installed("rlang")) {
        # rlang is not installed, no option except to use the base handler
        return(handle_error_base(cnd))
    }
    if (!inherits(cnd, "rlang_error") && !positron_option_error_entrace()) {
        # We have a non-rlang error, but the user requested we dont entrace it
        return(handle_error_base(cnd))
    }

    if (!inherits(cnd, "rlang_error")) {
        cnd <- rlang::catch_cnd(rlang::entrace(cnd))

        if (is.null(cnd)) {
            # We expect `entrace()` to always signal a condition since we:
            # - Know we are providing a non-rlang error condition
            # - Already handled the `utils::recover` case above
            # But we try to be defensive here anyways.
            return()
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
}

#' @param traceback A list of calls.
format_traceback <- function(calls = list()) {
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

    if (!.ps.is_installed("rlang", "1.1.1.9000")) {
        # In older versions of rlang, rlang did not respect `show.error.messages`
        # and there was no way to keep it from printing to the console. To work
        # around this, we throw a dummy base error after recording the rlang information.
        # Nicely, this:
        # - Won't print due to `show.error.messages = FALSE`
        # - Prevents rlang from printing its own error
        # However, this:
        # - Causes `traceback()` to show the global calling handler frames
        # - Causes `options(error = recover)` to show the global calling handler frames
        stop("dummy")
    }
}

# See `rlang:::has_recover()`
has_recover_handler <- function() {
    handler <- getOption("error")

    if (!is_call(handler)) {
        return(FALSE)
    }

    identical(handler[[1L]], utils::recover)
}

positron_option_error_entrace <- function() {
    # TODO: Wire this up to a Positron option for easy toggling?
    isTRUE(getOption("positron.error_entrace", default = TRUE))
}

rust_backtrace <- function() {
    .ps.Call("ps_rust_backtrace")
}
