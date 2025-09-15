#' @export
.rs.api.showDialog <- function(title, message, url = "") {
    if (!identical(url, "")) {
        url <- sprintf('<a href="%s">%s</a>', url, url)
        message <- sprintf('%s<br>%s', message, url)
    }
    .ps.ui.showDialog(title, message)
}

#' @export
.rs.api.showQuestion <- function(title, message, ok = NULL, cancel = NULL) {
    .ps.ui.showQuestion(title, message, ok, cancel)
}


#' @export
.rs.api.showPrompt <- function(title, message, default) {
    # rstudioapi doesn't pass the timeout directly but sets it as
    # `rstudioapi.remote.timeout` option
    timeout <- getOption('rstudioapi.remote.timeout', 60L)

    # validate args
    if (!nzchar(title)) {
        stop("Title must be a non-empty string")
    }
    if (!nzchar(message)) {
        stop("Message must be a non-empty string")
    }
    .ps.ui.showPrompt(
        as.character(title),
        as.character(message),
        default,
        as.integer(timeout)
    )
}
