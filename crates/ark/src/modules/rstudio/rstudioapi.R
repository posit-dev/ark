#' @export
.rs.api.versionInfo <- function() {

    current_year <- format(Sys.Date(), "%Y")
    positron_citation <-
        utils::bibentry(
            "Manual",
            title = "Positron: A next generation data science IDE",
            author = utils::person("Posit team"),
            organization = "Posit Software, PBC",
            address      = "Boston, MA",
            year         = current_year,
            url          = "https://www.posit.co/",
            textVersion =
                paste(
                    paste0("Posit team (", current_year, "). "),
                    "Positron: A next generation data science IDE. ",
                    "Posit Software, PBC, Boston, MA. ",
                    "URL https://www.posit.co/.",
                    sep = ""
                ),
            mheader = "To cite Positron in publications use:",
            mfooter = ""
        )
    class(positron_citation) <- c("citation", "bibentry")

    list(
        citation = positron_citation,
        mode = .rs.api.getMode(),
        version = .rs.api.getVersion(),
        long_version = Sys.getenv("POSITRON_LONG_VERSION"),
        ark_version = .ps.ark.version()
    )
}

# Since rstudioapi 0.17.0
#' @export
.rs.api.getVersion <- function() {
    package_version(Sys.getenv("POSITRON_VERSION"))
}

# Since rstudioapi 0.17.0
#' @export
.rs.api.getMode <- function() {
    Sys.getenv("POSITRON_MODE")
}
