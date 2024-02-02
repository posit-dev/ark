is_macos <- function() {
    Sys.info()[["sysname"]] == "Darwin"
}

has_aqua <- function() {
    is_macos() && capabilities("aqua")
}
has_cairo <- function() {
    capabilities("cairo")
}
has_x11 <- function() {
    capabilities("X11")
}
