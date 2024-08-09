
#' @export
.ps.rpc.reticulate_interpreter_path <- function() {
    if (!.ps.is_installed("reticulate")) return("")
    reticulate::py_discover_config()$python
}
