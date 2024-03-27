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

    ps_ranges <- lapply(location, rstudioapi::document_range)
    invisible(.ps.ui.insertText(ranges, text))
    list(ranges = ranges, text = text, id = NULL)
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
        selection = convert_positron_selection(context$selections)
    )
}

# Positron selection --> RStudio selection
convert_positron_selection <- function(ps_sels) {
    convert_one <- function(ps_sel) {
        list(
            range = rstudioapi::document_range(
                start = convert_positron_position(ps_sel$start),
                end = convert_positron_position(ps_sel$end)
            ),
            text = ps_sel$text
        )
    }
    out <- lapply(ps_sels, convert_one)
    rstudioapi:::as.document_selection(out)
}

# Positron position --> RStudio position
convert_positron_position <- function(ps_pos) {
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
.rs.api.setSelectionRanges <- function(ranges, id = NULL) {
    # TODO: Support document IDs
    stopifnot(is.null(id))

    ranges <- validate_ranges(ranges)
    .ps.ui.setSelectionRanges(unlist(ranges))
    invisible(list(
        ranges = ranges,
        id = id
    ))
}

## similar to "validateAndTransformLocation" from
## https://github.com/rstudio/rstudio/blob/main/src/cpp/r/R/Api.R
## (takes care of converting to zero-based indexing)
validate_ranges <- function(location) {
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
    invisible(.ps.ui.executeCommand("workbench.action.files.saveAll"))
}

#' @export
.rs.api.documentSave <- function(id = NULL) {
    # TODO: Support document IDs
    stopifnot(is.null(id))

    invisible(.ps.ui.executeCommand("workbench.action.files.save"))
}
