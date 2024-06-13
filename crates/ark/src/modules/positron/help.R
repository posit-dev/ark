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

#' Start R's dynamic HTTP help server; returns the chosen port (invisibly)
#'
#' If the help server is already started, the port already in use is returned
#' (due to using `start = NA`).
#' @export
.ps.help.startOrReconnectToHelpServer <- function() {
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

  package_root <- find_package_root(x$path)
  macros <- load_macros(package_root)

  # - `no_links = TRUE` because we don't click links while looking at docs on hover
  # - `dynamic = FALSE` because we want the HTML to be static for the hover provider, it isn't connected to a help server
  tools::Rd2HTML(
    x$path,
    out = path,
    package = x$pkg,
    stages = x$stage,
    no_links = TRUE,
    dynamic = FALSE,
    macros = macros
  )

  contents <- readLines(path, warn = FALSE)
  paste(contents, collapse = "\n")
}

#' @export
.ps.help.previewRd <- function(rd_file) {
  # `/preview` causes this to be handled by preview_rd() in ark's help proxy.
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
  if (!is.null(package_root) && !nzchar(package)) {
    package_desc <- package_info(package_root)
    package <- package_desc$Package %||% ""
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
  )

  # Make tweaks to the returned HTML
  lines <- readLines(path, warn = FALSE)

  lines <- sub(
    "R Documentation</td></tr></table>",
    "(preview) R Documentation</td></tr></table>",
    lines
  )

  # This condition is meant to detect the scenario where we are clearly working
  # inside the source of a specific in-development package, which will almost
  # always be true.
  if (!is.null(package_root) && nzchar(package)) {
    # `/dev-figure` causes this to be handled by preview_img() in ark's help proxy.
    lines <- gsub(
      'img src="figures/([^"]*)"',
      sprintf('img src="dev-figure?file=%s/man/figures/\\1"', package_root),
      lines
    )

    # Rewrite links to other help topics.
    lines <- vapply(lines, rewrite_help_links, "", package, package_root)
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

# @param path The path to a file believed to be inside an R source package.
#   Currently only used when we expect a path like
#   {package-root}/man/some_topic.Rd. But you could imagine doing a more general
#   recursive walk upwards, if we ever need that.
# @returns Normalized path to package root or NULL if we don't seem to be in a
#   package.
find_package_root <- function(path) {
  maybe_package_root <- dirname(dirname(path))

  if (file.exists(file.path(maybe_package_root, "DESCRIPTION"))) {
    normalizePath(maybe_package_root)
  } else {
    NULL
  }
}

# @param path Normalized path to package root.
# @returns A list containing the metadata in DESCRIPTION.
package_info <- function(path) {
  stopifnot(is_string(path))

  desc <- file.path(path, "DESCRIPTION")
  if (!file.exists(desc)) {
    stop("No DESCRIPTION found")
  }

  desc_mat <- read.dcf(desc)
  as.list(desc_mat[1, ])
}

# @param line A single line of HTML in a rendered help topic.
# @param package The name of the in-development package.
# @param package_root The normalized path to the source of the in-development
#   package.
# @returns The input `line`, with its help links rewritten.
rewrite_help_links <- function(line, package, package_root) {
  # inspired by an official example for regmatches()
  # gregexec() returns overlapping ranges: the first match is the full match,
  #   then the sub-matches follow (pkg and topic, for us)
  # when we use the `regmatches<-` assignment form, we want to keep only the
  #   coordinates for the first, full match
  keep_first <- function(x) {
    if(!anyNA(x) && all(x > 0)) {
      ml <- attr(x, 'match.length')
      it <- attr(x, 'index.type')
      ub <- attr(x, 'useBytes')
      if(is.matrix(x)) {
        x <- x[1, , drop = FALSE]
      } else {
        x <- x[1]
      }
      if(is.matrix(ml)) {
        attr(x, 'match.length') <- ml[1, , drop = FALSE]
      } else {
        attr(x, 'match.length') <- ml[1]
      }
      attr(x, 'index.type') <- it
      attr(x, 'useBytes') <- ub
    }
    x
  }

  # Accomplishes two goals:
  # 1. Determine if topic belongs to the in-development package.
  # 2. If so, account for the scenario where our nominal `topic` is actually an
  #    alias and we need the actual topic name.
  dev_topic_find <- function(topic) {
    # It should be exceedingly rare, but if we somehow get here in the absence
    # of pkgload, all bets are off re: rewriting links.
    if (!.ps.is_installed("pkgload")) {
      return(NA_character_)
    }

    tf <- pkgload::dev_topic_find(topic, dev_packages = package)
    if (is.null(tf)) {
      NA_character_
    } else {
      tf$path
    }
  }

  # Incoming links look like this:
  # href="../../PACKAGE/help/TOPIC"
  #
  # For a topic known to be in this OTHER_PACKAGE, we rewrite as:
  # href="/library/OTHER_PACKAGE/help/TOPIC"
  #
  # For a topic in this in-development PACKAGE, we rewrite like:
  # href="/preview?file=/normalized/path/to/source/of/PACKAGE/man/TOPIC.Rd"
  #
  # When we know the topic is not in this in-development PACKAGE, but we don't
  # know which other package it belongs to, we rewrite as:
  # href="/library/PACKAGE/help/TOPIC"
  # This is obviously wrong, but this is how we delegate the problem of
  # resolving the package back to the R help server.

  # concrete incoming examples:
  #                        dev  a href="../../devhelp/help/blarg">
  #   installed, known package  a href="../../rlang/help/abort">
  # installed, unknown package  a href="../../devhelp/help/match">
  pattern <- 'a href="../../(?<pkg>[^/]*)/help/(?<topic>[^/]*)">'

  x <- gregexec(pattern, line, perl = TRUE)
  rm <- regmatches(line, x)[[1]]

  if (length(rm) == 0) {
    return(line)
  }

  match_data  <- as.data.frame(t(rm))

  maybe_dev_rd_path <- vapply(match_data$topic, dev_topic_find, "")

  replacement <- ifelse(
    is.na(maybe_dev_rd_path),
    sprintf('a href="/library/%s/help/%s">', match_data$pkg, match_data$topic),
    sprintf('a href="/preview?file=%s">', maybe_dev_rd_path)
  )
  regmatches(line, lapply(x, keep_first)) <- list(as.matrix(replacement))

  # concrete outgoing examples:
  #                        dev  a href="/preview?file=/Users/jenny/rrr/devhelp/man/blarg.Rd">
  #   installed, known package  a href="/library/rlang/help/abort">
  # installed, unknown package  a href="/library/devhelp/help/match">

  line
}
