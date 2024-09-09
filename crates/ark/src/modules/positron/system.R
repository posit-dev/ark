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

#' Reports aspects of the locale for the R process.
#' @returns Named character vector of LANG env var and all categories of the locale.
#' @export
.ps.rpc.get_locale <- function() {
    cats <- .LC.categories
    stats::setNames(cats, cats)
    out <- as.list(vapply(cats, Sys.getlocale, "string", USE.NAMES = TRUE))
    c(LANG = Sys.getenv("LANG"), out)
}

#' Reports a vector of environment variables for the R process.
#' @returns Named character vector of env vars that start with the given regex.
#' @export
.ps.rpc.get_env_vars <- function(pattern = NULL) {
  ev <- as.list(Sys.getenv())
  m <- if (is.null(pattern)) TRUE else grep(pattern, names(ev))
  ev[m]
}
