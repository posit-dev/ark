# Invariants:
#
# - Return value: List of length 2 [output, error]. These are exclusive.
#
# - Error slot: Character vector of length 2 [message, trace], with `trace`
#   possibly an empty string.
#
# TODO: Prevent this function from jumping with on.exit
safe_evalq <- function(expr, env) {
    # Create a promise to make call stack leaner
    do.call(delayedAssign, list("out", substitute(expr), env))

    # Prepare non-local exit with error value
    err <- NULL
    delayedAssign("bail", return(list(NULL, err)))

    handler <- function(cnd) {
        # Save backtrace in error value
        calls <- sys.calls()
        trace <- paste(format(calls), collapse = '\n')

        message <- conditionMessage(cnd)

        # A character vector is easier to destructure from Rust.
        err <<- c(message, trace)

        # Trigger non-local return
        force(bail)
    }

    withCallingHandlers(
        list(out, NULL),
        interrupt = handler,
        error = handler
    )
}
