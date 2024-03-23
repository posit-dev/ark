#
# s3_override.R
#
# Copyright (C) 2023 Posit Software, PBC. All rights reserved.
#
#


# Original copies of S3 methods that we've overridden.
s3_originals <- new.env(parent = emptyenv())

# Our overrides of S3 methods.
s3_overrides <- new.env(parent = emptyenv())

# Override an S3 method.
#
# Ported from RStudio's similarly named method.
add_s3_override <- function(name, method) {
   # get a reference to the table of S3 methods stored in the base namespace
   table <- .BaseNamespaceEnv[[".__S3MethodsTable__."]]

   # cache old dispatch table entry if it exists
   if (exists(name, envir = table)) {
      assign(name, get(name, envir = table), envir = s3_originals)
   }

   # add a flag indicating that this method belongs to us
   attr(method, "positron.s3_override") <- TRUE

   # ... and inject our own entry
   assign(name, method, envir = table)

   # make a copy in our override table so we can restore when overwritten by e.g. an attached
   # package
   assign(name, method, envir = s3_overrides)

   invisible(NULL)
}

# Stop overriding an S3 method.
#
# Ported from RStudio's similarly named method.
remove_s3_override <- function(name) {
   table <- .BaseNamespaceEnv[[".__S3MethodsTable__."]]

   # see if there's an override to remove; if not, no work to do
   if (!exists(name, envir = table))
      return(invisible(NULL))

   # see if the copy that exists in the methods table is one that we put there.
   if (!isTRUE(attr(get(name, envir = table), "positron.s3_override", exact = TRUE)))
   {
      # it isn't, so don't touch it. we do this so that changes to the S3 dispatch table that
      # have occurred since the call to s3_override are persisted
      return(invisible(NULL))
   }

   # see if we have a copy to restore
   if (exists(name, envir = s3_originals))
   {
      # we do, so overwrite with our copy
      assign(name, get(name, envir = s3_originals), envir = table)
   }
   else
   {
      # no copy to restore, so just remove from the dispatch table
      rm(list = name, envir = table)
   }

   # remove from our override table if present
   if (exists(name, envir = s3_overrides))
      rm(list = name, envir = s3_overrides)

   invisible(NULL)
}

# Recovers from changes made to the S3 method dispatch table during e.g. package load
reattach_S3_overrides <- function() {
   # get a list of all of the methods that are currently overridden
   names <- ls(envir = s3_overrides)
   table <- .BaseNamespaceEnv[[".__S3MethodsTable__."]]
   for (name in names) {
      if (exists(name, envir = table)) {
         # retrieve reference to method
         method = get(name, envir = table)

         # if we didn't put the method there, we've been replaced; reattach our own method.
         if (!isTRUE(attr(get(name, envir = table), ".positron.s3_override", exact = TRUE)))
            add_s3_override(name, get(name, envir = s3_overrides))
      }
   }
}
