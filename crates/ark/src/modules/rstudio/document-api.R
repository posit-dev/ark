#' @export
.rs.api.getActiveDocumentContext <- function() {
    .rs.api.getSourceEditorContext(NULL)
}

#' @export
.rs.api.getSourceEditorContext <- function(id = NULL) {
    # TODO: Support document IDs
    stopifnot(is.null(id))

    context <- .ps.ui.LastActiveEditorContext()

    if (is.null(context)) {
      return()
    }

    list(
        path = context$document$path,
        contents = unlist(context$contents),
        selection = convert_selection(context$selections)
    )
}

# Positron selection --> RStudio selection
convert_selection <- function(ps_sels) {
    convert_one <- function(ps_sel) {
        list(
            range = rstudioapi::document_range(
                start = convert_position(ps_sel$start),
                end = convert_position(ps_sel$end)
            ),
            text = ps_sel$text
        )
    }
    out <- lapply(ps_sels, convert_one)
    rstudioapi:::as.document_selection(out)
}

# Positron position --> RStudio position
convert_position <- function(ps_pos) {
    with(
        ps_pos,
        rstudioapi::document_position(
            row = line + 1,
            column = character + 1
        )
    )
}

#' @export
.rs.api.documentNew <- function(text,
                                type = c("r", "rmarkdown", "sql"),
                                position = rstudioapi::document_position(0, 0),
                                execute = FALSE) {
    type <- match.arg(type)
    # TODO: Support execute & position
    stopifnot(!execute && position != rstudioapi::document_position(0, 0))

    languageId <- if (type == "rmarkdown") "rmd" else type
    invisible(.ps.ui.documentNew(text, languageId))
}

#' @export
.rs.api.documentSaveAll <- function() {
    # This excludes untitled files in RStudio
    invisible(.ps.ui.executeCommand("workbench.action.files.saveAllTitled"))
}

#' @export
.rs.api.documentSave <- function(id = NULL) {
    # TODO: Support document IDs
    stopifnot(is.null(id))

    invisible(.ps.ui.executeCommand("workbench.action.files.save"))
}
