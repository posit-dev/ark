log_warning <- function(msg) {
    stopifnot(is_string(msg))
    .Call("harp_log_warning", msg)
}

log_error <- function(msg) {
    stopifnot(is_string(msg))
    .Call("harp_log_error", msg)
}

is_string <- function(x) {
    is.character(x) && length(x) == 1 && !is.na(x)
}

is_matrix <- function(x) {
    length(dim(x)) == 2 && !is.data.frame(x)
}

class_collapsed <- function(x) {
    paste0(class(x), collapse = "/")
}
