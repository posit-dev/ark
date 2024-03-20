#' @export
.rs.api.showDialog <- function(title, message, url = "") {
    url <- sprintf('<a href="%s">%s</a>', url, url)
    message <- sprintf('%s<br>%s', message, url)
    .ps.ui.showDialog(title, message)
}

#' @export
.rs.api.showQuestion <- function(title, message, ok = NULL, cancel = NULL) {
    .ps.ui.showQuestion(title, message, ok, cancel)
}
