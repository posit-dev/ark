#
# help.R
#
# Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
#
#

options(help_type = "html")

# A wrapper around `help()` that works for our specific use cases:
# - Picks up devtools `help()` if the shim is on the search path.
# - Expects that `topic` and `package` don't require NSE and are just strings or `NULL`.
# - Works around a pkgload NSE bug that has been fixed, but many people won't have
#   (https://github.com/r-lib/pkgload/pull/267).
# - Hardcodes a request for HTML help results.
help <- function(topic, package = NULL) {
  if ("devtools_shims" %in% search()) {
      help <- as.environment("devtools_shims")[["help"]]
  } else {
      help <- utils::help
  }

  # Since `topic` and `package` are strings (or `NULL`), we wrap them in `()` to tell the
  # special NSE semantics of `help()` to evaluate them rather than deparse them.
  if (is.null(package)) {
    # Use an explicit `NULL` to ensure this always works with dev help
    # https://github.com/r-lib/pkgload/pull/267
    help(topic = (topic), package = NULL, help_type = "html")
  } else {
    help(topic = (topic), package = (package), help_type = "html")
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
    info <- split_topic(topic)
    topic <- info$topic
    package <- info$package

    # Try to find help on the topic.
    results <- help(topic, package)

    # If we found results of any kind, show them.
    # If we are running ark tests, don't show the results as this requires
    # `ps_browse_url()` which needs a full `RMain` instance.
    if (length(results) > 0 && !in_ark_tests()) {
        print(results)
    }

    # Return whether we found any help.
    length(results) > 0
}

# Resolve the package specifier, if there is one
split_topic <- function(topic) {
    # Try `:::` first, as `::` will match both
    components <- strsplit(topic, ":::")[[1L]]
    if (length(components) > 1L) {
        package <- components[[1L]]
        topic <- components[[2L]]
        return(list(topic = topic, package = package))
    }

    components <- strsplit(topic, "::")[[1L]]
    if (length(components) > 1L) {
        package <- components[[1L]]
        topic <- components[[2L]]
        return(list(topic = topic, package = package))
    }

    list(topic = topic, package = NULL)
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
  helpFiles <- help(topic, package)

  if (inherits(helpFiles, "dev_topic")) {
    getHtmlHelpContentsDev(helpFiles)
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

getHtmlHelpContentsDev <- function(x) {
  tryCatch(
    getHtmlHelpContentsDevImpl(x),
    error = function(e) NULL
  )
}

# pkgload specific dev help when looking up help for an internal function
# while working on a package
getHtmlHelpContentsDevImpl <- function(x) {
  if (!"pkgload" %in% loadedNamespaces()) {
    # Refuse if we somehow get a dev topic but pkgload isn't loaded
    return(NULL)
  }

  directory <- positron_tempdir("help")
  path <- file.path(directory, "dev-contents.html")

  # Would be great it pkgload exposed this officially.
  # Possibly as `topic_write(x, path, type = c("text", "html"))`.
  # Also used by RStudio in the exact same way.
  pkgload:::topic_write_html(x = x, path = path)

  contents <- readLines(path, warn = FALSE)
  paste(contents, collapse = "\n")
}

#' @export
.ps.help.previewRd <- function(rd_file) {
  # I have allowed myself to use pkgload in the implementation of this, because
  # the main usage we anticipate is via the pkgload shims for `help()` and `?`.
  # This could maybe be relaxed to a check whether pkgload is installed.
  # The usage doesn't absolutely require that it be attached.
  if (!"pkgload" %in% loadedNamespaces()) {
    return(NULL)
  }

  # Get "devhelp" and "foofy" out of a path like:
  # /Users/jenny/rrr/devhelp/man/foofy.Rd
  pkg <- basename(dirname(dirname(rd_file)))
  topic <- tools::file_path_sans_ext(basename(rd_file))

  # Get the help files associated with this topic.
  help_files <- help(topic, pkg)

  # Prepare a temporary filepath
  file <- paste0(topic, ".html")
  doc_path <- file.path("doc", "html", file)
  # This MUST have a very specific form to please the R help server.
  # In particular, I can't use positron_tempdir() here.
  directory <- file.path(tempdir(), ".R", "doc", "html")
  if (!dir.exists(directory)) {
    if (!dir.create(directory, showWarnings = FALSE, recursive = TRUE)) {
      stop(sprintf("Can't create temporary directory at '%s'.", directory))
    }
  }
  html_path <- file.path(directory, file)
  # `html_path` looks like:
  # /tmp/RtmpDOCyeE/.R/doc/html/foofy.html

  pkgload_topic_write_html(x = help_files, path = html_path)

  # fixups from RStudio's Rd2HTML function
  # https://github.com/rstudio/rstudio/blob/eef4efa0b4a9a6c6d984912a09cd6504decfb8c6/src/cpp/session/modules/SessionHelp.R#L1021
  lines <- readLines(html_path, warn = FALSE)
  lines <- sub(
    "R Documentation</td></tr></table>",
    "(preview) R Documentation</td></tr></table>",
    lines
  )
  if (nzchar(pkg)) {
      # replace with "dev-figure" and parameters so that the server
      # can look for figures in `man/` of the dev package
      lines <- sub(
        'img src="figures/([^"]*)"',
        sprintf('img src="dev-figure?pkg=%s&figure=\\1"',pkg),
        lines
      )

      # add ?dev=<topic>
      lines <- gsub(
        'a href="../../([^/]*/help/)([^/]*)">',
        'a href="/library/\\1\\2?dev=\\2">',
        lines
      )
  }
  writeLines(lines, html_path)

  # This MUST be a localhost URL for Positron to open it in the help pane.
  port <- pkgload:::httpdPort()
  url <- sprintf("http://127.0.0.1:%i/%s", port, doc_path)

  utils::browseURL(url)
}

pkgload_topic_write_html <- function(x, path) {
  macros <- pkgload:::load_rd_macros(dirname(dirname(x$path)))

  tools::Rd2HTML(
    x$path,
    out = path,
    package = x$pkg,
    stages = x$stage,
    no_links = TRUE,
    macros = macros
  )

  # departure from pkgload:::topic_write_html()
  # make sure R.css is a sibling to path
  css_path <- file.path(dirname(path), "R.css")
  # departure from pkgload:::topic_write_html()
  # TODO: use Positron's R.css instead of the one that ships with R?
  if (!file.exists(css_path)) {
    file.copy(file.path(R.home("doc"), "html", "R.css"), css_path)
  }
}
