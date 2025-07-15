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
    if (!requireNamespace(package_name, quietly = TRUE)) {
        return(paste("Package", package_name, "is not installed."))
    }

    # Search for help topics in the package
    help_db <- utils::help.search(
        "",
        package = package_name,
        fields = c("alias", "title"),
        ignore.case = TRUE
    )
    res <- help_db$matches

    # Did we get any matches?
    if (nrow(res) == 0) {
        return(paste("No help topics found for package", package_name, "."))
    }

    res_split <- split(res, res$Name)
    res_list <- lapply(res_split, function(group) {
        list(
            topic_id = group$Name[1],
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
        if (identical(pkg, "base") || is_on_disk(pkg)) {
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
    if (!requireNamespace(package_name, quietly = TRUE)) {
        return(paste("Package", package_name, "is not installed."))
    }

    # Get vignettes for the package
    vignettes <- tools::getVignetteInfo(package = package_name)
    if (length(vignettes) == 0) {
        return(paste("Package", package_name, "has no vignettes."))
    }

    # Convert the matrix to a list of lists
    vignette_list <- lapply(seq_len(nrow(vignettes)), function(i) {
        list(
            title = vignettes[i, "Title"],
            topic = vignettes[i, "Topic"]
        )
    })
    vignette_list
}

#' Get a specific vignette for a package
#'
#' This function retrieves a specific vignette available for a specified package in R.
#' It returns the vignette content as a Markdown character string.
#'
#' Adapted from btw::btw_tool_docs_vignette.
#'
#' @param package_name Name of the package to get vignettes for
#' @return A list of vignettes for the package, each with a title and topic.
#'
#' @export
.ps.rpc.get_package_vignette <- function(package_name, vignette) {
    vignettes <- as.data.frame(tools::getVignetteInfo(package = package_name))
    if (nrow(vignettes) == 0) {
        return(paste("Package", package_name, "has no vignettes."))
    }
    vignette_info <- vignettes[vignettes$Topic == vignette, , drop = FALSE]
    if (nrow(vignette_info) == 0) {
        return(
            paste(
                "No vignette",
                vignette,
                "for package",
                package_name,
                "found."
            )
        )
    }

    # Use Pandoc (bundled with Positron) to convert rendered vignette (PDF or
    # HTML) to Markdown
    output_file <- tempfile(fileext = ".md")
    tryCatch(
        {
            pandoc_convert(
                input = file.path(vignette_info$Dir, "doc", vignette_info$PDF),
                to = "markdown",
                output = output_file,
                verbose = FALSE
            )
            # read the converted Markdown file
            vignette_md <- readLines(output_file, warn = FALSE)

            # remove the first line which is the title
            vignette_md <- vignette_md[-1]
            vignette_md <- paste(vignette_md, collapse = "\n")
            vignette_md
        },
        error = function(e) {
            paste("Error converting vignette:", e$message)
        }
    )
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
        if (!requireNamespace(package_name, quietly = TRUE)) {
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
        calls <- sprintf(
            '{"topic":"%s", "package_name":"%s"}',
            resolved$resolved,
            resolved$package
        )
        calls <- stats::setNames(calls, "*")
        return(
            paste(
                "Topic",
                topic,
                "matched",
                length(resolved$resolved),
                "different topics. Choose one or submit individual tool calls for each topic.",
            )
        )
    }

    # Convert the help page to Markdown using Pandoc
    md_file <- tempfile(fileext = ".md")
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

    # Add a heading for the help page
    heading <- sprintf(
        "## `help(package = \"%s\", \"%s\")`",
        resolved$package,
        topic
    )

    # Return the help page as a list
    list(
        help_text = paste0(md, collapse = "\n"),
        topic = basename(resolved$topic),
        package = resolved$package
    )
}
