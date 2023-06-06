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
    handlers <- c(handlers, list(error = .ps.errors.globalErrorHandler))
    do.call(globalCallingHandlers, handlers)

    # Tell rlang and base R not to print the error message, we will do it!
    options(show.error.messages = FALSE)

    invisible(NULL)
}

.ps.errors.globalErrorHandler <- function(cnd) {    
    if (!is_installed("rlang", "1.1.1.9000")) {
        # rlang is not installed or is not new enough,
        # no option except to use the base handler
        return(handle_error_base(cnd))
    }
    
    if (!inherits(cnd, "rlang_error")) {
        if (!positron_option_error_entrace()) {
            # We have a non-rlang error, but the user requested we dont entrace it
            return(handle_error_base(cnd))
        }
      
        # TODO: Is `rlang::cnd_entrace()` an option?
        # Lionel mentioned something against it.
        cnd <- rlang::catch_cnd(rlang::entrace(cnd))
    }
    
    handle_error_rlang(cnd)
}