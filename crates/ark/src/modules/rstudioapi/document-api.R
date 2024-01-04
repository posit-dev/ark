.rs.api.getActiveDocumentContext <- function() {
    getSourceEditorContext(NULL)
}

.rs.api.getSourceEditorContext <- function(id = NULL) {
    # TODO: Support document IDs
    stopifnot(is.null(id))

    context <- .ps.getActiveDocumentContext()

    list(
        path = context$path,

        # TODO: These fields are empty stubs just to make
        # `getSourceEditorContext()` work without erroring
        contents = character(),
        selection = selection()
    )
}

# Creates an empty selection
selection <- function() {
    rstudioapi:::as.document_selection(
        list(list(range = c(0, 0, 0, 0), text = ""))
    )
}
