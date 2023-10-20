#
# help.R
#
# Copyright (C) 2023 Posit Software, PBC. All rights reserved.
#
#

options(help_type = "html")

# Start R's dynamic HTTP help server; returns the chosen port (invisibly)
.ps.help.startHelpServer <- function() {
    suppressMessages(tools::startDynamicHelp(start = NA))
}

# Show help on a topic. Returns a logical value indicating whether help was
# found.
.ps.help.showHelpTopic <- function(topic) {
    # Try to find help on the topic.
    results <- help(topic)

    # If we found results of any kind, show them.
    if (length(results) > 0) {
        print(results)
    }

    # Return whether we found any help.
    length(results) > 0
}

.ps.help.getHtmlHelpContents <- function(topic, package = "") {

  # If a package name is encoded into 'topic', split that here.
  if (grepl(":{2,3}", topic)) {
    parts <- strsplit(topic, ":{2,3}")[[1L]]
    package <- parts[[1L]]
    topic <- parts[[2L]]
  }

  # Get the help file associated with this topic.
  helpFiles <- help(topic = (topic), package = if (nzchar(package)) package)
  if (length(helpFiles) == 0)
    return(NULL)

  # Get the help documentation.
  helpFile <- helpFiles[[1L]]
  rd <- utils:::.getHelpFile(helpFile)

  # Set 'package' now if it was unknown.
  if (identical(package, "")) {
    pattern <- "/library/([^/]+)/"
    m <- regexec(pattern, helpFile, perl = TRUE)
    matches <- regmatches(helpFile, m)
    if (length(matches) && length(matches[[1L]] == 2L))
      package <- matches[[1L]][[2L]]
  }

  # Convert to html.
  htmlFile <- tempfile(fileext = ".html")
  on.exit(unlink(htmlFile), add = TRUE)
  tools::Rd2HTML(rd, out = htmlFile, package = package)
  contents <- readLines(htmlFile, warn = FALSE)
  paste(contents, collapse = "\n")

}
