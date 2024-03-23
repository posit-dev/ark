#
# calls.R
#
# Copyright (C) 2024 Posit Software, PBC. All rights reserved.
#
#

# Based on `rlang::call_name()`
call_name <- function(x) {
  if (typeof(x) != "language") {
    return(NULL)
  }

  switch(
    call_type(x),
    named = as.character(x[[1L]]),
    namespaced = as.character(x[[1L]][[3L]]),
    NULL
  )
}

call_type <- function(x) {
  stopifnot(typeof(x) == "language")

  node <- x[[1L]]
  type <- typeof(node)

  if (type == "symbol") {
    "named"
  } else if (is_namespaced_symbol(node)) {
    "namespaced"
  } else if (type == "language") {
    "recursive"
  } else if (type %in% c("closure", "builtin", "special")) {
    "inlined"
  } else {
    stop("corrupt language object")
  }
}

is_namespaced_symbol <- function(x) {
  if (typeof(x) != "language") {
    return(FALSE)
  }

  node <- x[[1L]]

  identical(node, as.name("::")) || identical(node, as.name(":::"))
}
