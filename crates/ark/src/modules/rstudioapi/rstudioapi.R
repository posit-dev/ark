#' @export
.rs.api.versionInfo <- function() {
    # comments are what rstudioapi actually does
    # https://github.com/rstudio/rstudio/blob/bb729e14867f6f95e26600d8b38e4551402cf7de/src/cpp/r/R/Api.R#L135-L145
    # info <- list()
    list(
        # info$citation <- .Call("rs_rstudioCitation", PACKAGE = "(embedding)")
        citation = NULL,
        # info$mode <- .Call("rs_rstudioProgramMode", PACKAGE = "(embedding)")
        mode = "desktop",
        # info$edition <- .Call("rs_rstudioEdition", PACKAGE = "(embedding)")
        # info$version <- .Call("rs_rstudioVersion", PACKAGE = "(embedding)")
        # info$version <- package_version(info$version)
        version = package_version("2023.3"),
        # info$long_version <- .Call("rs_rstudioLongVersion", PACKAGE = "(embedding)")
        long_version = "2023.03.0"
        # info$release_name <- .Call("rs_rstudioReleaseName", PACKAGE = "(embedding)")
    )
    #     info
}
