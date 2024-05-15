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
  # This `/preview` url gets special handling in `proxy_request()`.
  url <- sprintf("/preview?file=%s", rd_file)
  port <- tools:::httpdPort()
  url <- tools:::dynamicHelpURL(url, port)
  .ps.Call("ps_browse_url", as.character(url))
}

# @param rd_file Path to an `.Rd` file.
# @returns The result of converting that `.Rd` to HTML and concatenating to a
#   string.
#' @export
.ps.Rd2HTML <- function(rd_file, package = "") {
  package_root <- find_package_root(rd_file)
  package_desc <- package_info(package_root)

  if (!nzchar(package) && utils::hasName(package_desc, "Package")) {
    package <- package_desc$Package
  }

  path <- tempfile(fileext = ".html")
  on.exit(unlink(path), add = TRUE)

  # Write HTML to file (with support for links and dynamic requests)
  macros <- load_macros(package_root)
  tools::Rd2HTML(
    rd_file,
    out = path,
    package = package,
    macros = macros,
    dynamic = TRUE
    # TO THINK: could we use the stylesheet argument to inject our own R.css?
  )

  # Make tweaks to the returned HTML
  lines <- readLines(path, warn = FALSE)

  lines <- sub(
    "R Documentation</td></tr></table>",
    "(preview) R Documentation</td></tr></table>",
    lines
  )

  if (nzchar(package)) {
    # TODO(Jenny) support dev-figure
    # Replace with "dev-figure" and parameters so that `proxy_request()` of our help proxy
    # server can look for figures in `man/` of the dev package.
    lines <- sub(
      'img src="figures/([^"]*)"',
      sprintf('img src="dev-figure?pkg=%s&figure=\\1"', package),
      lines
    )

    # TODO(Jenny) support `?dev` query parameter. Likely by calling
    # `pkgload::dev_topic_find()` and then recalling `rd_as_html()` with that Rd file
    # and the returned `package`.
    # Two purposes:
    # - For non-dev topics, these end up correctly going through `/library/` again rather
    #   than looking into a temp directory.
    # - For dev topics, the `?dev=<topic>` query parameter gives us a chance to try to look
    #   up the dev topic ourselves before forwarding on to the R server, i.e. in
    #   `proxy_request()` of our help proxy server.
    lines <- gsub(
      'a href="../../([^/]*/help/)([^/]*)">',
      'a href="/library/\\1\\2?dev=\\2">',
      lines
    )
  }

  paste0(lines, collapse = "\n")
}

load_macros <- function(package_root) {
  if (is.null(package_root) ||
      !file.exists(file.path(package_root, "DESCRIPTION"))) {
    path <- file.path(R.home("share"), "Rd/macros/system.Rd")
    tools::loadRdMacros(path)
  } else {
    tools::loadPkgRdMacros(package_root)
  }
}

# @param path The path to file believed to be inside an R source package.
#   Currently only used when we expect a path like
#   {package-root}/man/some_topic.Rd. But you could imagine doing a more general
#   recursive walk upwards, if we ever need that.
# @returns Normalized path to package root or NULL if we don't seem to be in a
#   package.
find_package_root <- function(path) {
  # currently hard-wired for the case of finding package root from an Rd filepath
  # i.e. we anticipate input that's
  # could use a more general recurisve walk upwards, if we ever have that need
  maybe_package_root <- dirname(dirname(path))

  if (file.exists(file.path(maybe_package_root, "DESCRIPTION"))) {
    normalizePath(maybe_package_root)
  } else {
    NULL
  }
}

# @param path Normalized path to package root or, possibly, NULL.
# @returns A list containing the metadata in DESCRIPTION or, for NULL input or
#   when no DESCRIPTION is found, an empty list.
package_info <- function(path) {
  if (is.null(path)) {
    return(list())
  }

  desc <- file.path(path, "DESCRIPTION")
  if (!file.exists(desc)) {
    return(list())
  }

  desc_mat <- read.dcf(desc)
  as.list(desc_mat[1, ])
}
