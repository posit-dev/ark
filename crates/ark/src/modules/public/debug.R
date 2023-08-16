.ps.debug.stackInfo <- function() {
    stack <- sys.calls()
    stack <- stack[-length(stack)]

    lapply(stack, .ps.debug.frameInfo)
}

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

  # Deparse expression in function position to use as name. This is not
  # necessarily a symbol, it could be a complex expression. TODO: Include
  # namespace information as in rlang backtraces.
  name <- deparse(call[[1]])

  # If a multiline expression we just truncate it
  name <- name[[1]]

  list(
    name = name,
    file = file,
    line = line,
    column = column
  )
}
