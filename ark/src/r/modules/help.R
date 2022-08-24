#
# help.R
#
# Copyright (C) 2022 by RStudio, PBC
#
#

.rs.getHelpText <- function(package, topic) {

    # get the help files
    # the parentheses below are included so we can dodge R's NSE handling
    topic <- help((topic), (package), verbose = FALSE)

    # get the .Rd documentation for this topic
    rd <- utils:::.getHelpFile(topic)

    # convert help to text
    pattern <- sprintf("%s-%s-", package, topic)
    output <- tools::Rd2txt(rd, out = tempfile(pattern = pattern), package = package)

    # read the contents of that file
    contents <- readLines(output, warn = FALSE)
    paste(contents, collapse = "\n")

}
