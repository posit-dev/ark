#' @export
.rs.api.showDialog <- function(title, message, url = "") {
    stopifnot(url == "")
    .ps.ui.showDialog(title, message)
}

#' @export
.rs.api.showQuestion <- function(title, message, ok = NULL, cancel = NULL) {
    .ps.ui.showQuestion(title, message, ok, cancel)
}
