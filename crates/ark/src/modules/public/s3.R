#
# s3.R
#
# Copyright (C) 2023 Posit Software, PBC. All rights reserved.
#
#

.ps.s3.genericNameCache <- new.env(parent = emptyenv())

.ps.s3.genericNameFromFunction <- function(callable) {

    # Check whether we can safely cache the result.
    isCacheable <- !is.null(packageName(environment(callable)))
    if (!isCacheable)
        return(.ps.s3.genericNameFromFunctionImpl(callable))

    id <- .ps.objectId(callable)
    .ps.s3.genericNameCache[[id]] <-
        .ps.s3.genericNameCache[[id]] %??%
        .ps.s3.genericNameFromFunctionImpl(callable)

}

.ps.s3.genericNameFromFunctionImpl <- function(callable) {

    useMethodSym <- as.name("UseMethod")
    value <- .ps.recursiveSearch(body(callable), function(node) {
        if (is.call(node) &&
            length(node) >= 2L &&
            identical(node[[1L]], useMethodSym) &&
            is.character(node[[2L]]))
        {
            return(node[[2L]])
        }
    })

    as.character(value)

}

# Original copies of S3 methods that we've overridden.
.ps.S3Originals <- new.env(parent = emptyenv())

# Our overrides of S3 methods.
.ps.S3Overrides <- new.env(parent = emptyenv())

# Override an S3 method.
#
# Ported from RStudio's similarly named method.
.ps.s3.addS3Override <- function(name, method) {
   # get a reference to the table of S3 methods stored in the base namespace
   table <- .BaseNamespaceEnv[[".__S3MethodsTable__."]]

   # cache old dispatch table entry if it exists
   if (exists(name, envir = table)) {
      assign(name, get(name, envir = table), envir = .ps.S3Originals)
   }

   # add a flag indicating that this method belongs to us
   attr(method, ".ps.S3Override") <- TRUE

   # ... and inject our own entry
   assign(name, method, envir = table)

   # make a copy in our override table so we can restore when overwritten by e.g. an attached
   # package
   assign(name, method, envir = .ps.S3Overrides)

   invisible(NULL)
}

# Stop overriding an S3 method.
#
# Ported from RStudio's similarly named method.
.ps.s3.removeS3Override <- function(name) {
   table <- .BaseNamespaceEnv[[".__S3MethodsTable__."]]

   # see if there's an override to remove; if not, no work to do
   if (!exists(name, envir = table))
      return(invisible(NULL))

   # see if the copy that exists in the methods table is one that we put there.
   if (!isTRUE(attr(get(name, envir = table), ".ps.S3Override", exact = TRUE)))
   {
      # it isn't, so don't touch it. we do this so that changes to the S3 dispatch table that
      # have occurred since the call to .ps.addS3Override are persisted
      return(invisible(NULL))
   }

   # see if we have a copy to restore
   if (exists(name, envir = .ps.S3Originals))
   {
      # we do, so overwrite with our copy
      assign(name, get(name, envir = .ps.S3Originals), envir = table)
   }
   else
   {
      # no copy to restore, so just remove from the dispatch table
      rm(list = name, envir = table)
   }

   # remove from our override table if present
   if (exists(name, envir = .ps.S3Overrides))
      rm(list = name, envir = .ps.S3Overrides)

   invisible(NULL)
}

# Recovers from changes made to the S3 method dispatch table during e.g. package load
.ps.reattachS3Overrides <- function() {
   # get a list of all of the methods that are currently overridden
   names <- ls(envir = .ps.S3Overrides)
   table <- .BaseNamespaceEnv[[".__S3MethodsTable__."]]
   for (name in names) {
      if (exists(name, envir = table)) {
         # retrieve reference to method
         method = get(name, envir = table)

         # if we didn't put the method there, we've been replaced; reattach our own method.
         if (!isTRUE(attr(get(name, envir = table), ".ps.S3Override", exact = TRUE)))
            .ps.s3.addS3Override(name, get(name, envir = .ps.S3Overrides))
      }
   }
}
