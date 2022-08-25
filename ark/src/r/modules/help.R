#
# help.R
#
# Copyright (C) 2022 by RStudio, PBC
#
#

# TODO: Cache these so we can avoid re-creating help eagerly.
.rs.help.getHelpTextFromFile <- function(helpFile, package = "") {

    rd <- utils:::.getHelpFile(helpFile)

    output <- tempfile(pattern = "help-", fileext = ".txt")
    tools::Rd2txt(rd, out = output, package = package)

    contents <- readLines(output, warn = FALSE)
    paste(contents, collapse = "\n")

}

.rs.help.package <- function(package) {

    # First, check for a help topic called '<package>-package'
    topic <- sprintf("%s-package", package)
    helpFiles <- help(topic = (topic), package = (package))
    if (length(helpFiles)) {
        return(.rs.help.getHelpTextFromFile(helpFiles[[1L]]))
    }

    # Otherwise, generate a simple piece of help based on the package's DESCRIPTION file
    # TODO: NYI
    ""

}
