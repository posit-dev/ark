is_macos <- function() {
    Sys.info()[["sysname"]] == "Darwin"
}

has_aqua <- function() {
    is_macos() && capabilities("aqua")
}
