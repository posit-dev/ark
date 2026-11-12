#' @export
.rs.api.getActiveProject <- function() {
    invisible(.ps.ui.workspaceFolder())
}

#' @export
.rs.api.navigateToFile <- function(
    file = character(0),
    line = -1L,
    column = -1L,
    moveCursor = TRUE
) {
    # TODO: support moveCursor argument
    stopifnot(moveCursor)

    invisible(.ps.ui.navigateToFile(file, line, column))
}

#' @export
.rs.api.askForPassword <- function(prompt = "Please enter your password") {
    .ps.ui.askForPassword(prompt)
}

#' @export
.rs.api.sendToConsole <- function(
    code,
    echo = TRUE,
    execute = TRUE,
    focus = TRUE,
    animate = FALSE
) {
    # TODO: support other args
    stopifnot(echo && execute && !animate)

    # If we add new args later, remember to put them **after** the existing args
    invisible(.ps.ui.executeCode(paste(code, collapse = "\n"), focus))
}

#' @export
.rs.api.restartSession <- function(command = "") {
    # TODO: support followup `command` argument
    stopifnot(command == "")

    invisible(.ps.ui.executeCommand(
        'workbench.action.language.runtime.restartActiveSession'
    ))
}

#' @export
.rs.api.openProject <- function(path = NULL, newSession = FALSE) {
    path <- normalizePath(path)
    invisible(.ps.ui.openWorkspace(path, newSession))
}

#' @export
.rs.api.viewer <- function(url, height = NULL) {
    # Validate arguments
    if (!is.character(url) || (length(url) != 1)) {
        stop("url must be a single element character vector.")
    }
    height <- .ps.validate.viewer.height(height)

    if (!is_http_url(url)) {
        # Only normalize file path urls (posit-dev/positron#4843)
        url <- normalizePath(url, mustWork = FALSE)
    }

    # Derive a title for the viewer from the path.
    title <- .ps.viewer.title(url)

    invisible(.Call(
        "ps_html_viewer",
        url, # The URL of the file to view
        title, # The name of the file to display in the viewer
        height, # The desired height
        FALSE, # Whether the object is a plot; guess FALSE
        PACKAGE = "(embedding)"
    ))
}

#' @export
.rs.api.runScriptJob <- function(
    path,
    name = NULL,
    encoding = "unknown",
    workingDir = NULL,
    importEnv = FALSE,
    exportEnv = ""
) {
    if (missing(path)) {
        stop("Must specify path to R script to run.", call. = FALSE)
    }
    check_string <- function(x, arg) {
        if (!is_string(x)) {
            stop("`", arg, "` must be a single string.", call. = FALSE)
        }
    }
    check_string(path, "path")
    if (!is.null(name)) {
        check_string(name, "name")
    }
    check_string(encoding, "encoding")
    if (!is.null(workingDir)) {
        check_string(workingDir, "workingDir")
    }
    if (!file.exists(path) || dir.exists(path)) {
        stop("The R script '", path, "' does not exist.", call. = FALSE)
    }
    path <- normalizePath(path, winslash = "/")

    if (is.null(name) || !nzchar(name)) {
        name <- basename(path)
    }
    if (is.null(workingDir)) {
        workingDir <- dirname(path)
    }
    if (!dir.exists(workingDir)) {
        stop(
            "The working directory '",
            workingDir,
            "' does not exist.",
            call. = FALSE
        )
    }
    workingDir <- normalizePath(workingDir, winslash = "/")

    if (isTRUE(importEnv)) {
        warning(
            "`importEnv = TRUE` is not supported in Positron and is ignored.",
            call. = FALSE
        )
    }
    if (is_string(exportEnv) && nzchar(exportEnv)) {
        warning(
            "`exportEnv` is not supported in Positron and is ignored.",
            call. = FALSE
        )
    }

    # Braces make one expression so a failed setwd() stops the job before source()
    code <- sprintf(
        "{\n    setwd(%s)\n    source(%s, encoding = %s)\n}",
        deparse(workingDir),
        deparse(path),
        deparse(encoding)
    )

    invisible(.ps.ui.executeCodeInNewSession(code, name))
}
