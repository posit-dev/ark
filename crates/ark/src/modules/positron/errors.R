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
format_traceback <- function(traceback = list()) {
    n <- length(traceback)

    # TODO: This implementation prints the traceback in the same ordering
    # as rlang, i.e. with call 1 on the stack being the first thing you
    # see. `traceback()` prints in the reverse order, so users may want a
    # way to reverse both our display ordering and rlang's (requiring a
    # new `rlang::format()` argument).

    # Collect source location, if there is any
    srcrefs <- lapply(traceback, function(call) attr(call, "srcref"))
    srcrefs <- vapply(srcrefs, src_loc, FUN.VALUE = character(1))
    has_srcref <- nchar(srcrefs) != 0L
    srcrefs[has_srcref] <- vec_paste0(" at ", srcrefs[has_srcref])

    # Converts to a list of quoted calls to a list of deparsd calls.
    # Respects global options `"traceback.max.lines"` and `"deparse.max.lines"`!
    traceback <- .traceback(traceback)

    # Prepend the stack number to each deparsed call, padding multiline calls as needed,
    # and then collapse multiline calls into one line
    prefixes <- vec_paste0(seq_len(n), ". ")
    prefixes <- format(prefixes, justify = "right")

    traceback <- mapply(prepend_prefix, traceback, prefixes, SIMPLIFY = FALSE)
    traceback <- lapply(traceback, function(lines) paste0(lines, collapse = "\n"))
    traceback <- as.character(traceback)

    paste0(traceback, srcrefs)
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

src_loc <- function(srcref) {
    # Adapted from `rlang:::src_loc()`
    if (is.null(srcref)) {
        return("")
    }

    srcfile <- attr(srcref, "srcfile")
    if (is.null(srcfile)) {
        return("")
    }

    # May be:
    # - An actual file path
    # - `""` for user defined functions in the console
    # - `"<text>"` for `parse()`d functions
    # We only try and display the source location for file paths
    file <- srcfile$filename
    if (identical(file, "") || identical(file, "<text>")) {
        return("")
    }

    file_trimmed <- path_trim_prefix(file, 3L)

    first_line <- srcref[[1L]]
    first_column <- srcref[[5L]]

    # TODO: We could generate file hyperlinks here like `rlang:::src_loc()`
    paste0(file_trimmed, ":", first_line, ":", first_column)
}

path_trim_prefix <- function(path, n) {
    # `rlang:::path_trim_prefix()`
    split <- strsplit(path, "/")[[1]]
    n_split <- length(split)

    if (n_split <= n) {
        path
    } else {
        paste(split[seq(n_split - n + 1, n_split)], collapse = "/")
    }
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

positron_option_error_entrace <- function() {
    # TODO: Wire this up to a Positron option for easy toggling?
    isTRUE(getOption("positron.error_entrace", default = TRUE))
}

rust_backtrace <- function() {
    .ps.Call("ps_rust_backtrace")
}
