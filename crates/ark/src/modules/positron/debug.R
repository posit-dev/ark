#
# debug.R
#
# Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
#
#

#' @export
.ps.debug.stackInfo <- function() {
    stack <- sys.calls()
    stack <- stack[-length(stack)]

    lapply(stack, .ps.debug.frameInfo)
}

#' @export
.ps.debug.frameInfo <- function(call) {
  srcref <- attr(call, "srcref")

  if (is.null(srcref)) {
    return(NULL)
  }

  srcfile <- attr(srcref, "srcfile")
  if (is.null(srcfile)) {
    return(NULL)
  }

  file <- srcfile$filename
  if (identical(file, "") || identical(file, "<text>")) {
    return(NULL)
  }

  file <- normalizePath(file, mustWork = FALSE)
  line <- srcref[[1]]
  column <- srcref[[5]]

  is_int <- function(x) is.integer(x) && length(x) == 1 && !is.na(x)
  is_str <- function(x) is.character(x) && length(x) == 1 && !is.na(x)

  if (!is_str(file) || !is_int(line) || !is_int(column)) {
    return(NULL)
  }

  # Deparse expression to use as name. In R, frames do not have a
  # name. They only have a call site expression that we use as identifier.
  name <- deparse(call)

  # If a multiline expression we just truncate it
  name <- name[[1]]

  # TODO: Include namespace information as in rlang backtraces?
  list(
    name = name,
    file = file,
    line = line,
    column = column
  )
}
