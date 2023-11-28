#
# errors.R
#
# Copyright (C) 2023 Posit Software, PBC. All rights reserved.
#
#

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

.ps.errors.globalErrorHandler <- function(cnd) {
    # Don't instrument errors if the option has been switched back on
    if (isTRUE(getOption("show.error.messages", TRUE))) {
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
    }

    handle_error_rlang(cnd)
}

.ps.errors.globalMessageHandler <- function(cnd) {
    # Decline to handle if we can't muffle the message (should only happen
    # in extremely rare cases)
    if (is.null(findRestart("muffleMessage"))) {
        return()
    }

    msg <- conditionMessage(cnd)

    # Add trailing newline if there isn't one, including for the length 0
    # case. See https://github.com/posit-dev/positron/issues/1878.
    #
    # This is to accomodate base message which may or may not have an
    # explicit trailing newline, see the `appendLF` argument of
    # `message()`. Because other message emitters might conform to the
    # behaviour of `warning()` and `stop()` which never add newlines, we
    # have to account for both possibility.
    #
    # Because of this we lose support for constructing messages within a
    # single line piece by piece with `message(appendLF = FALSE)`, but
    # that's an esoteric feature. If this is reported, we could do more
    # specialised behaviour depending on whether the message condition is
    # inherits from `"simpleMessage"` or not.
    n <- nchar(msg)
    if (substr(msg, n, n) != "\n") {
        msg <- paste0(msg, "\n")
    }

    # Output the condition message to the relevant stream (normally
    # stdout). Note that for historical reasons, messages include a
    # trailing newline
    cat(msg, file = default_message_file())

    # Silence default message handling
    invokeRestart("muffleMessage")
}

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
