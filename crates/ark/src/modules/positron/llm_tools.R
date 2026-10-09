#
# llm_tools.R
#
# Copyright (C) 2025 Posit Software, PBC. All rights reserved.
#
#

#' Get the help topics for a package
#'
#' This function retrieves the help topics for a specified package in R.
#' It returns a data frame with the topic ID, title, and aliases for each help
#' topic in the package.
#'
#' Adapted from btw::btw_tool_docs_package_help_topics
#'
#' @param package_name Name of the package to get help topics for
#' @return A list of help topics for the package, each with a topic ID,
#'   title, and aliases.
#'
#' @export
.ps.rpc.list_package_help_topics <- function(package_name) {
    # Check if the package is installed
    if (!is_on_disk(package_name)) {
        return(paste("Package", package_name, "is not installed."))
    }

    topics <- package_help_topics(package_name)
    if (!length(topics)) {
        return(paste("No help topics found for package", package_name, "."))
    }
    lapply(topics, function(topic) {
        list(
            topic_id = topic$topic,
            title = topic$title,
            aliases = topic$aliases
        )
    })
}

#' List the documentation for a package
#'
#' Returns the help topics and vignettes for a package, so that individual
#' pages can be read with `get_help_page()` and `get_package_vignette()`.
#'
#' @param package_name Name of the package
#' @return A list with the package name, its help topics (each with a topic,
#'   title, and aliases), and its vignettes (each with a name and title), or
#'   a message if the package is not installed.
#'
#' @export
.ps.rpc.list_package_docs <- function(package_name) {
    if (!is_on_disk(package_name)) {
        return(paste("Package", package_name, "is not installed."))
    }

    list(
        package = package_name,
        topics = package_help_topics(package_name),
        vignettes = lapply(package_vignettes(package_name), function(info) {
            list(name = info$Topic, title = info$Title)
        })
    )
}

#' Adapted from btw::btw_tool_docs_package_help_topics
package_help_topics <- function(package_name) {
    help_db <- utils::help.search(
        "",
        package = package_name,
        fields = c("alias", "title"),
        ignore.case = TRUE
    )
    res <- help_db$matches

    res_split <- split(res, res$Name)
    res_list <- lapply(res_split, function(group) {
        list(
            topic = group$Name[1],
            title = group$Entry[group$Field == "Title"][1],
            aliases = paste(
                group$Entry[group$Field == "alias"],
                collapse = ", "
            )
        )
    })
    names(res_list) <- NULL
    res_list
}

#' The vignettes of a package, as a list of rows of `tools::getVignetteInfo()`
package_vignettes <- function(package_name) {
    vignettes <- as.data.frame(tools::getVignetteInfo(package = package_name))
    lapply(seq_len(nrow(vignettes)), function(i) as.list(vignettes[i, ]))
}

#' Get the version of installed packages
#'
#' This function retrieves the versions of specified packages in R.
#'
#' It returns a named list where the names are the package names and the values
#' are the corresponding package versions, or "Not installed" if the package is
#' not found.
#'
#' @export
.ps.rpc.get_package_versions <- function(package_names, ...) {
    lapply(set_names(package_names), function(pkg) {
        if (is_on_disk(pkg)) {
            as.character(utils::packageVersion(pkg))
        } else {
            "Not installed"
        }
    })
}

#' Get available vignettes for a package
#'
#' This function retrieves the vignettes available for a specified package in R.
#' It returns a list of vignettes, each with a title and topic.
#'
#' Adapted from btw::btw_tool_docs_available_vignettes.
#'
#' @param package_name Name of the package to get vignettes for
#' @return A list of vignettes for the package, each with a title and topic.
#'
#' @export
.ps.rpc.list_available_vignettes <- function(package_name) {
    # Check if the package is installed
    if (!is_on_disk(package_name)) {
        return(paste("Package", package_name, "is not installed."))
    }

    vignettes <- package_vignettes(package_name)
    if (!length(vignettes)) {
        return(paste("Package", package_name, "has no vignettes."))
    }

    lapply(vignettes, function(info) {
        list(title = info$Title, topic = info$Topic)
    })
}

#' Get a specific vignette for a package
#'
#' Returns the vignette as Markdown, converted with Pandoc from its rendered
#' HTML. Vignettes without rendered HTML (e.g. PDF vignettes) are returned as
#' their source (R Markdown, Sweave, etc.).
#'
#' Adapted from btw::btw_tool_docs_vignette.
#'
#' @param package_name Name of the package
#' @param vignette Name of the vignette
#' @return A list with the vignette's `content`, `title`, `name`, and
#'   `package`, or a message if the vignette is not found.
#'
#' @export
.ps.rpc.get_package_vignette <- function(package_name, vignette) {
    if (!is_on_disk(package_name)) {
        return(paste("Package", package_name, "is not installed."))
    }

    vignettes <- package_vignettes(package_name)
    if (!length(vignettes)) {
        return(paste("Package", package_name, "has no vignettes."))
    }
    vignette_names <- vapply(vignettes, function(info) info$Topic, character(1))
    info <- vignettes[vignette_names == vignette]
    if (!length(info)) {
        return(paste0(
            "No vignette ",
            vignette,
            " found for package ",
            package_name,
            ". Available vignettes: ",
            paste(vignette_names, collapse = ", "),
            "."
        ))
    }
    info <- info[[1]]

    content <- tryCatch(
        read_vignette(info),
        error = function(err) err
    )
    if (inherits(content, "error")) {
        return(paste("Error reading vignette:", conditionMessage(content)))
    }

    # Drop embedded images, which are large and unreadable as text
    content <- gsub("!\\[[^]]*\\]\\(data:[^)]*\\)", "", content)

    list(
        content = content,
        title = info$Title,
        name = info$Topic,
        package = package_name
    )
}

read_vignette <- function(info) {
    doc_dir <- file.path(info$Dir, "doc")
    if (grepl("\\.html?$", info$PDF, ignore.case = TRUE)) {
        output_file <- tempfile(fileext = ".md")
        on.exit(unlink(output_file), add = TRUE)
        pandoc_convert(
            input = file.path(doc_dir, info$PDF),
            to = "markdown_strict-raw_html+pipe_tables+backtick_code_blocks",
            output = output_file
        )
        content <- readLines(output_file, warn = FALSE, encoding = "UTF-8")
    } else {
        content <- readLines(
            file.path(doc_dir, info$File),
            warn = FALSE,
            encoding = "UTF-8"
        )
    }
    paste(content, collapse = "\n")
}

#' Get a specific help page
#'
#' This function retrieves a specific help page available for a specified package in R.
#' It returns the help page content as a Markdown character string.
#'
#' Adapted from btw::btw_tool_docs_help_page.
#'
#' @param topic The topic to get help for
#' @param package_name The name of the package to get help for. If empty,
#' searches all installed packages.
#' @return A list of help pages for the package, each with a title and topic.
#'
#' @export
.ps.rpc.get_help_page <- function(topic, package_name = "") {
    if (identical(package_name, "")) {
        package_name <- NULL
    }

    if (!is.null(package_name)) {
        if (!is_on_disk(package_name)) {
            return(paste("Package", package_name, "is not installed."))
        }
    }

    # Temporarily disable menu graphics
    old.menu.graphics <- getOption("menu.graphics", default = TRUE)
    options(menu.graphics = FALSE)
    on.exit(options(menu.graphics = old.menu.graphics), add = TRUE)

    # Read the help page
    help_page <- utils::help(
        package = (package_name),
        topic = (topic),
        help_type = "text",
        try.all.packages = (is.null(package_name))
    )

    if (!length(help_page)) {
        return(
            paste0(
                "No help page found for topic ",
                topic,
                if (!is.null(package_name)) {
                    paste(" in package", package_name)
                } else {
                    " in all installed packages"
                },
                "."
            )
        )
    }

    # Resolve the help page to a specific topic and package
    resolved <- help_package_topic(help_page)

    if (length(resolved$resolved) > 1) {
        matches <- paste0(resolved$package, "::", resolved$resolved)
        return(
            paste0(
                "Topic ",
                topic,
                " matched ",
                length(matches),
                " help pages: ",
                paste(matches, collapse = ", "),
                ". Specify the package to choose one."
            )
        )
    }

    # Convert the help page to Markdown using Pandoc
    md_file <- tempfile(fileext = ".md")
    on.exit(unlink(md_file), add = TRUE)
    format_help_page_markdown(
        help_page,
        output = md_file,
        options = c("--shift-heading-level-by=1")
    )
    md <- readLines(md_file, warn = FALSE)

    # Remove up to the first empty line
    first_empty <- match(TRUE, !nzchar(md), nomatch = 1) - 1
    if (first_empty > 0) {
        md <- md[-seq_len(first_empty)]
    }

    # Return the help page as a list
    list(
        help_text = paste0(md, collapse = "\n"),
        topic = basename(resolved$topic),
        package = resolved$package
    )
}
