#
# pandoc.R
#
# Copyright (C) 2025 Posit Software, PBC. All rights reserved.
#

# This file provides functions to convert documents using the pandoc utility.
# Most are ported from the rmarkdown package and adapted for use in Positron.

#' Convert a document with pandoc
#'
#' Convert documents to and from various formats using the pandoc utility.
#'
#' Supported input and output formats are described in the
#' \href{https://pandoc.org/MANUAL.html}{pandoc user guide}.
#'
#' The system path as well as the version of pandoc shipped with Positron
#' are scanned for pandoc and the highest version available is used.
#'
#' Adapted from rmarkdown::pandoc_convert()
#'
#' @param input Character vector containing paths to input files
#'   (files must be UTF-8 encoded)
#' @param to Format to convert to (if not specified, you must specify
#'   \code{output})
#' @param from Format to convert from (if not specified then the format is
#'   determined based on the file extension of \code{input}).
#' @param output Output file (if not specified then determined based on format
#'   being converted to).
#' @param options Character vector of command line options to pass to pandoc.
#' @param verbose \code{TRUE} to show the pandoc command line which was executed
#' @param wd Working directory in which code will be executed. If not
#'   supplied, defaults to the common base directory of \code{input}.
#' @examples
#' \dontrun{
#'
#' # convert markdown to various formats
#' pandoc_convert("input.md", to = "html")
#' pandoc_convert("input.md", to = "latex")
#'
#' # add some pandoc options
#' pandoc_convert("input.md", to = "latex", options = c("--listings"))
#' }
pandoc_convert <- function(
    input,
    to = NULL,
    from = NULL,
    output = NULL,
    options = NULL,
    verbose = FALSE,
    wd = NULL
) {
    # evaluate path arguments before changing working directory
    force(output)

    # execute in specified working directory
    if (is.null(wd)) {
        wd <- dirname(input)
    }

    oldwd <- setwd(wd)
    on.exit(setwd(oldwd), add = TRUE)

    # input file and formats
    args <- pandoc_path_arg(input)
    if (!is.null(to)) {
        if (to == 'html') {
            to <- 'html4'
        }
        if (to == 'pdf') {
            to <- 'latex'
        }
        args <- c(args, "--to", to)
    }
    if (!is.null(from)) {
        args <- c(args, "--from", from)
    }

    # output file
    if (!is.null(output)) {
        args <- c(args, "--output", pandoc_path_arg(output))
    }

    # additional command line options
    args <- c(args, options)

    # ensure pandoc is available
    pandoc_info <- find_pandoc()
    if (is.null(pandoc_info$dir) || !utils::file_test("-x", pandoc())) {
        stop(
            "pandoc is not available. Please install pandoc or set the ",
            "RSTUDIO_PANDOC environment variable to the directory containing ",
            "the pandoc executable."
        )
    }

    # build the conversion command
    command <- paste(
        quoted(file.path(pandoc_info$dir, "pandoc")),
        paste(quoted(args), collapse = " ")
    )

    # show it in verbose mode
    if (verbose) {
        cat(command, "\n")
    }

    # run the conversion
    with_pandoc_safe_environment({
        result <- system(command)
    })
    if (result != 0) {
        stop("pandoc document conversion failed with error ", result)
    }

    invisible(NULL)
}

#' Convert path arguments for pandoc
#'
#' Adapted from rmarkdown:::pandoc_path_arg()
pandoc_path_arg <- function(path, backslash = TRUE) {
    path <- path.expand(path)
    path <- sub("^[.]/", "", path)
    i <- grepl("^-", path) & xfun::is_rel_path(path)
    path[i] <- paste0("./", path[i])
    if (identical(.Platform$OS.type, "windows")) {
        i <- grep(" ", path)
        if (length(i)) {
            path[i] <- utils::shortPathName(path[i])
        }
        if (backslash) {
            path <- gsub("/", "\\\\", path)
        }
    }
    path
}

#' Quote arguments for shell commands
#'
#' Adapted from rmarkdown:::quoted()
quoted <- function(args) {
    shell_chars <- grepl("[ <>()|\\:&;#?*']", args)
    args[shell_chars] <- shQuote(args[shell_chars])
    args
}

#' Get the path to the pandoc executable
pandoc <- function() {
    pandoc_info <- find_pandoc()
    if (is.null(pandoc_info$dir)) {
        return(NULL)
    }
    build_pandoc_path(pandoc_info$dir)
}

#' Test if a directory exists
dir_exists <- function(x) {
    length(x) > 0 && utils::file_test('-d', x)
}

#' Find the pandoc executable in the system path or specified directory
find_pandoc <- function(dir = NULL, version = NULL) {
    # look up pandoc in potential sources unless user has supplied `dir`
    sources <- if (length(dir) == 0) {
        c(
            Sys.getenv("RSTUDIO_PANDOC"),
            dirname(find_program("pandoc")),
            "~/opt/pandoc"
        )
    } else {
        dir
    }
    sources <- path.expand(sources)

    # determine the versions of the sources
    versions <- lapply(sources, function(src) {
        if (dir_exists(src)) get_pandoc_version(src) else numeric_version("0")
    })

    # find the maximum version
    found_src <- NULL
    found_ver <- numeric_version("0")
    for (i in seq_along(sources)) {
        ver <- versions[[i]]
        if (
            (!is.null(version) && ver == version) ||
                (is.null(version) && ver > found_ver)
        ) {
            found_ver <- ver
            found_src <- sources[[i]]
        }
    }

    list(
        dir = found_src,
        version = found_ver
    )
}

# Build the full path to the pandoc executable, handling Windows .exe suffix
build_pandoc_path <- function(pandoc_dir) {
    path <- file.path(pandoc_dir, "pandoc")
    if (identical(.Platform$OS.type, "windows")) {
        path <- paste0(path, ".exe")
    }
    path
}

# Find a program within the PATH. On OSX we need to explictly call
# /usr/bin/which with a forwarded PATH since OSX Yosemite strips
# the PATH from the environment of child processes
#
# Ported from rmarkdown::find_program()
find_program <- function(program) {
    if (Sys.info()["sysname"] == "Darwin") {
        res <- suppressWarnings({
            # Quote the path (so it can contain spaces, etc.) and escape any quotes
            # and escapes in the path itself
            sanitized_path <- gsub(
                "\\",
                "\\\\",
                Sys.getenv("PATH"),
                fixed = TRUE
            )
            sanitized_path <- gsub("\"", "\\\"", sanitized_path, fixed = TRUE)
            system(
                paste0(
                    "PATH=\"",
                    sanitized_path,
                    "\" /usr/bin/which ",
                    program
                ),
                intern = TRUE
            )
        })
        if (length(res) == 0) {
            ""
        } else {
            res
        }
    } else {
        Sys.which(program)
    }
}

# wrap a system call to pandoc so that LC_ALL is not set
# see: https://github.com/rstudio/rmarkdown/issues/31
# see: https://ghc.haskell.org/trac/ghc/ticket/7344
with_pandoc_safe_environment <- function(code) {
    lc_all <- Sys.getenv("LC_ALL", unset = NA)

    if (!is.na(lc_all)) {
        Sys.unsetenv("LC_ALL")
        on.exit(Sys.setenv(LC_ALL = lc_all), add = TRUE)
    }

    lc_ctype <- Sys.getenv("LC_CTYPE", unset = NA)

    if (!is.na(lc_ctype)) {
        Sys.unsetenv("LC_CTYPE")
        on.exit(Sys.setenv(LC_CTYPE = lc_ctype), add = TRUE)
    }

    if (
        Sys.info()['sysname'] == "Linux" &&
            is.na(Sys.getenv("HOME", unset = NA))
    ) {
        stop(
            "The 'HOME' environment variable must be set before running Pandoc."
        )
    }

    if (
        Sys.info()['sysname'] == "Linux" &&
            is.na(Sys.getenv("LANG", unset = NA))
    ) {
        # fill in a the LANG environment variable if it doesn't exist
        Sys.setenv(LANG = detect_generic_lang())
        on.exit(Sys.unsetenv("LANG"), add = TRUE)
    }

    if (
        Sys.info()['sysname'] == "Linux" &&
            identical(Sys.getenv("LANG"), "en_US")
    ) {
        Sys.setenv(LANG = "en_US.UTF-8")
        on.exit(Sys.setenv(LANG = "en_US"), add = TRUE)
    }

    force(code)
}

# Get an S3 numeric_version for the pandoc utility at the specified path
get_pandoc_version <- function(pandoc_dir) {
    path <- build_pandoc_path(pandoc_dir)
    if (!utils::file_test("-x", path)) {
        return(numeric_version("0"))
    }
    info <- with_pandoc_safe_environment(
        system(paste(shQuote(path), "--version"), intern = TRUE)
    )
    version <- strsplit(info, "\n", useBytes = TRUE)[[1]][1]
    version <- strsplit(version, " ")[[1]][2]
    components <- strsplit(version, "-")[[1]]
    version <- components[1]
    # pandoc nightly adds -nightly-YYYY-MM-DD to last release version
    # https://github.com/jgm/pandoc/issues/8016
    # mark it as devel appending YYYY.MM.DD
    nightly <- match("nightly", components)
    if (!is.na(nightly)) {
        version <- paste(
            c(
                version,
                grep("^[0-9]+$", components[-(1:nightly)], value = TRUE)
            ),
            collapse = "."
        )
    }
    numeric_version(version)
}
