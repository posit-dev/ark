#
# help.R
#
# Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
#
#

options(help_type = "html")

# Pick up `help()` devtools if the shim is on the search path.
# Internally, we should be using `get_help()` to work around a pkgload bug
# consistently.
help <- function(...) {
    if ("devtools_shims" %in% search()) {
        help <- as.environment("devtools_shims")[["help"]]
    } else {
        help <- utils::help
    }

  # Passing arguments with `...` avoids issues of NSE interpretation
  help(...)
}

# Expect that `topic` and `package` don't require NSE and are just strings or `NULL`.
get_help <- function(topic, package = NULL) {
  # Due to a pkgload NSE bug, we use an explicit `NULL` to ensure this always works with
  # dev help https://github.com/r-lib/pkgload/pull/267.
  # The `topic` and `package` are wrapped in `()` so they are evaluated rather than deparsed.
  if (is.null(package)) {
    help(topic = (topic), package = NULL)
  } else {
    help(topic = (topic), package = (package))
  }
}

# Start R's dynamic HTTP help server; returns the chosen port (invisibly)
#' @export
.ps.help.startHelpServer <- function() {
    suppressMessages(tools::startDynamicHelp(start = NA))
}

# Show help on a topic. Returns a logical value indicating whether help was
# found.
#' @export
.ps.help.showHelpTopic <- function(topic) {
    # Resolve the package specifier.
    package <- NULL
    components <- strsplit(topic, "::")[[1L]]
    if (length(components) > 1L) {
        package <- components[[1L]]
        topic <- components[[2L]]
    }

    # Try to find help on the topic.
    results <- get_help(topic, package)

    # If we found results of any kind, show them.
    # If we are running ark tests, don't show the results as this requires
    # `ps_browse_url()` which needs a full `RMain` instance.
    if (length(results) > 0 && !in_ark_tests()) {
        print(results)
    }

    # Return whether we found any help.
    length(results) > 0
}

# Expose the show help topic function as an RPC.
#' @export
.ps.rpc.showHelpTopic <- .ps.help.showHelpTopic

# Show a vignette. Returns a logical value indicating whether the vignette
# was found.
#' @export
.ps.rpc.showVignetteTopic <- function(topic) {
    # Resolve the package specifier.
    package <- NULL
    components <- strsplit(topic, "::")[[1L]]
    if (length(components) > 1L) {
        package <- components[[1L]]
        topic <- components[[2L]]
    }

    # Try to find the vignette; suppress warnings so we don't pollute the
    # console.
    results <- suppressWarnings(vignette(topic, package = package))

    # If we found a vignette, show it.
    if ("vignette" %in% class(results)) {
        print(results)
        TRUE
    } else {
        FALSE
    }
}

#' @export
.ps.help.getHtmlHelpContents <- function(topic, package = NULL) {

  # If a package name is encoded into 'topic', split that here.
  if (grepl(":{2,3}", topic)) {
    parts <- strsplit(topic, ":{2,3}")[[1L]]
    package <- parts[[1L]]
    topic <- parts[[2L]]
  }

  # Get the help file associated with this topic.
  helpFiles <- get_help(topic, package)

  if (inherits(helpFiles, "dev_topic")) {
    tryGetHtmlHelpContentsDev(helpFiles)
  } else {
    getHtmlHelpContentsInstalled(helpFiles, package)
  }
}

getHtmlHelpContentsInstalled <- function(helpFiles, package) {
  if (length(helpFiles) == 0) {
    return(NULL)
  }

  helpFile <- helpFiles[[1L]]

  rd <- utils:::.getHelpFile(helpFile)

  # Set 'package' now if it was unknown.
  if (is.null(package)) {
    pattern <- "/library/([^/]+)/"
    m <- regexec(pattern, helpFile, perl = TRUE)
    matches <- regmatches(helpFile, m)
    if (length(matches) && length(matches[[1L]] == 2L))
      package <- matches[[1L]][[2L]]
  }

  # If still unknown, set to `""` for `Rd2HTML()`
  if (is.null(package)) {
    package <- ""
  }

  # Convert to html.
  htmlFile <- tempfile(fileext = ".html")
  on.exit(unlink(htmlFile), add = TRUE)
  tools::Rd2HTML(rd, out = htmlFile, package = package)
  contents <- readLines(htmlFile, warn = FALSE)
  paste(contents, collapse = "\n")
}

tryGetHtmlHelpContentsDev <- function(x) {
  tryCatch(
    getHtmlHelpContentsDev(x),
    error = function(e) NULL
  )
}

# pkgload specific dev help when looking up help for an internal function
# while working on a package
getHtmlHelpContentsDev <- function(x) {
  if (!"pkgload" %in% loadedNamespaces()) {
    # Refuse if we somehow get a dev topic but pkgload isn't loaded
    return(NULL)
  }

  dir <- file.path(tempdir(), ".R", "doc", "html")
  dir.create(dir, recursive = TRUE, showWarnings = FALSE)

  path <- file.path(dir, sprintf("%s.html", x$topic))

  # Use pkgload to write out topic html. Calls `tools::Rd2HTML()` with
  # some extra features.
  pkgload:::topic_write_html(x, path = path)

  contents <- readLines(path, warn = FALSE)
  paste(contents, collapse = "\n")
}
