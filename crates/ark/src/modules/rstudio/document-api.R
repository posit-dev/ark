#' @export
.rs.api.modifyRange <- function(location = NULL,
                                text = NULL,
                                id = NULL) {
    .rs.api.insertText(location, text, id)
}

#' @export
.rs.api.insertText <- function(location = NULL,
                               text = NULL,
                               id = NULL) {
    # TODO: Support document IDs
    stopifnot(is.null(id))

    # For compatibility with calls like `rstudioapi::insertText("foo")`
    if (is.null(text)) {
        text <- location
        context <- .ps.ui.LastActiveEditorContext()
        location <- lapply(context$selections, selection_as_range)
    }

    ranges <- asRangeList(location)
    if (length(text) == 1L) {
        text <- rep(text, length(ranges))
    }
    stopifnot(length(text) == length(ranges))

    .ps.ui.modifyEditorSelections(ranges, text)
    invisible(list(
        ranges = ranges,
        text = text,
        id = id
    ))
}



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

selection_as_range <- function(ps_sel) {
    c(
        ps_sel$start$line,
        ps_sel$start$character,
        ps_sel$end$line,
        ps_sel$end$character
    )
}

#' @export
.rs.api.documentNew <- function(type, code, row = 0, column = 0, execute = FALSE) {
    # TODO: Support execute
    stopifnot(!execute)

    languageId <- if (type == "rmarkdown") "rmd" else type
    invisible(.ps.ui.newDocument(paste(code, collapse = "\n"), languageId, row, column))
}

#' @export
.rs.api.setSelectionRanges <- function(ranges, id = NULL) {
    # TODO: Support document IDs
    stopifnot(is.null(id))

    ranges <- asRangeList(ranges)
    .ps.ui.setSelectionRanges(lapply(ranges, function(x) x + 1L))
    invisible(list(
        ranges = ranges,
        id = id
    ))
}

# Similar to "validateAndTransformLocation" from
# https://github.com/rstudio/rstudio/blob/main/src/cpp/r/R/Api.R
#
# Here `location` is a (list of) `rstudioapi::document_position` or
# `rstudioapi::document_range` object(s), or numeric vectors coercable to such
# objects.
#
# Returns a list of length-four integer vectors representing ranges, like:
# [[1]]
# [1] 0 0 1 1
#
# [[2]]
# [1] 2 7 2 9
#
# [[3]]
# [1] 9 0 9 1
asRangeList <- function(location) {
    # allow a single range (then validate that it's a true range after)
    if (!is.list(location) || inherits(location, "document_range")) {
        location <- list(location)
    }

    ranges <- lapply(location, function(el) {
        # detect proxy Inf object
        if (identical(el, Inf)) {
            el <- c(Inf, 0, Inf, 0)
        }

        # detect positions (2-element vectors) and transform them to ranges
        n <- length(el)
        if (n == 2 && is.numeric(el)) {
            el <- c(el, el)
        }

        # detect document_ranges and transform
        if (is.list(el) && all(c("start", "end") %in% names(el))) {
            el <- c(el$start, el$end)
        }

        # validate we have a range-like object
        if (length(el) != 4 || !is.numeric(el) || any(is.na(el))) {
            stop("'ranges' should be a list of 4-element integer vectors", call. = FALSE)
        }

        # transform out-of-bounds values appropriately
        el[el < 1] <- 1
        el[is.infinite(el)] <- NA

        # transform from 1-based to 0-based indexing for server
        result <- as.integer(el) - 1L

        # treat NAs as end of row / column
        result[is.na(result)] <- as.integer(2^31 - 1)
        result
    })

    ranges
}

#' @export
.rs.api.documentSaveAll <- function() {
    # This function excludes untitled files in RStudio:
    invisible(.ps.ui.executeCommand("workbench.action.files.saveAllTitled"))
}

#' @export
.rs.api.documentSave <- function(id = NULL) {
    # TODO: Support document IDs
    stopifnot(is.null(id))

    invisible(.ps.ui.executeCommand("workbench.action.files.save"))
}
