
#' @export
.ps.rpc.reticulate_interpreter_path <- function() {
    if (!.ps.is_installed("reticulate")) return("")
    reticulate::py_discover_config()$python
}

#' @export
.ps.reticulate_open <- function(input="") {
    .ps.Call("ps_reticulate_open", input)
}

#' Used by the front-end to install reticulate
#' @export
.ps.rpc.install_reticulate <- function() {
    tryCatch({
        utils::install.packages("reticulate")
        TRUE
    }, error = function(err) {
        FALSE
    })
}

#' @export
.ps.rpc.reticulate_start_kernel <- function(kernelPath, connectionFile, logFile, logLevel) {
    # We execute as interactive to allow reticulate to prompt for installation
    # of required environments and/or Python packages.
    local_options(interactive = TRUE)
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
        # Empty string means that no error happened.
        ""
    }, error = function(err) {
        conditionMessage(err)
    })
}
