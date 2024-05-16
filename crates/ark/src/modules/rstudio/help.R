#' @export
.rs.api.previewRd <- function(rdFile) {
    if (!is.character(rdFile) || (length(rdFile) != 1))
        stop("rdFile must be a single element character vector.")
    if (!file.exists(rdFile))
        stop("The specified rdFile ' ", rdFile, "' does not exist.")

    invisible(.ps.help.previewRd(rdFile))
}
