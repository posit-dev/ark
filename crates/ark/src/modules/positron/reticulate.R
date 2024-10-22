#' @export
.ps.reticulate_open <- function(input="") {
    .ps.Call("ps_reticulate_open", input)
}

.ps.reticulate_shutdown <- function() {
    .ps.Call("ps_reticulate_shutdown")
}

.ps.reticulate_open_comm <- function(start_runtime) {
    .ps.Call("ps_reticulate_open_comm", start_runtime)
}

#' Called by the front-end right before starting the reticulate session.
#'
#' At this point it should be fine to load Python if it's not loaded, and
#' check if it can be started and if necessary packages are installed.
#' @export
.ps.rpc.reticulate_check_prerequisites <- function() {

    # This should return a list with the following fields:
    # python: NULL or string
    # venv: NULL or string
    # ipykernel: NULL or string
    # error: NULL or string

    config <- tryCatch({
        reticulate::py_discover_config()
    }, error = function(err) {
        err
    })

    if (inherits(config, "error")) {
        # py_discover_config() can fail if the user forced a Python session
        # via RETICULATE_PYTHON, but this version doesn't exist.
        return(list(error = conditionMessage(config)))
    }

    if (is.null(config) || is.null(config$python)) {
        # The front-end will offer to install Python.
        return(list(python = NULL, error = NULL))
    }

    python <- config$python
    venv <- config$virtualenv

    # Check that python can be loaded, if it can't we will throw
    # an error, which is unrecoverable.
    config <- tryCatch({
        reticulate::py_config()
    }, error = function(err) {
        err
    })

    if (inherits(config, "error")) {
        return(list(python = python, venv = venv, error = conditionMessage(config)))
    }

    # Now check ipykernel
    ipykernel <- tryCatch({
        reticulate::py_module_available("ipykernel")
    }, error = function(err) {
        err
    })

    if (inherits(ipykernel, "error")) {
        return(list(python = python, venv = venv, error = conditionMessage(ipykernel)))
    }

    list(
        python = config$python,
        venv = venv,
        ipykernel = ipykernel,
        error = NULL
    )
}

#' @export
.ps.rpc.reticulate_start_kernel <- function(kernelPath, connectionFile, logFile, logLevel) {
    # Starts an IPykernel in a separate thread with information provided by
    # the caller.
    # It it's essentially executing the kernel startup script:
    # https://github.com/posit-dev/positron/blob/main/extensions/positron-python/python_files/positron/positron_language_server.py
    # and passing the communication files that Positron Jupyter's Adapter sets up.
    tryCatch({
        reticulate:::py_run_file_on_thread(
            file = kernelPath,
            args = c(
                "-f", connectionFile,
                "--logfile", logFile,
                "--loglevel", logLevel,
                "--session-mode", "console"
            )
        )

        # Open comm with the front-end.
        # The runtime is already starting, so we set `start_runtime` to FALSE.
        .ps.reticulate_open_comm(start_runtime = FALSE)

        # Empty string means that no error happened.
        ""
    }, error = function(err) {
        conditionMessage(err)
    })
}

# Called whenever a new comm is created to register a finalizer, making sure that
# the Python session is gracefully exitted before the R session ends.
# During execution we might have a few finalizers registered, but they are no-ops
# if the session is already closed.
reticulate_register_finalizer <- function() {
    # Make sure we call the shutdown function when the R session ends.
    # This allows Positron to shutdown the Reticulate Python session
    # before the R session is gone - which causes the LSP to crash.
    reg.finalizer(asNamespace("reticulate"), function(...) {
        .ps.reticulate_shutdown()
    }, onexit = TRUE)
}
