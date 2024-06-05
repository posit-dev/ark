#
# startup.R
#
# Copyright (C) 2024 Posit Software, PBC. All rights reserved.
#
#

.ps.source_r_profile <- function(path) {
  # Disallow both `readline()` and `menu()` when sourcing `.Rprofile`s. `?Startup`
  # explicitly forbids "interaction with the user during startup", and it can crash
  # ark because we aren't ready to handle `input_request`s yet.
  local_pkg_hook(
    pkg = "base",
    name = "readline",
    hook = hook_readline,
    hook_namespace = hook_readline
  )
  local_pkg_hook(
    pkg = "utils",
    name = "menu",
    hook = hook_menu,
    hook_namespace = hook_menu
  )

  # Source in the global env to mimic R.
  # Errors are captured by the caller and relayed to the user.
  sys.source(file = path, envir = globalenv())

  invisible(NULL)
}

# We have extremely special cased behavior for existing scripts that bootstrap an older
# version of renv that previously would attempt to use `readline()` on startup. When
# we detect this case, we return `"n"` from `readline()` rather than erroring just to
# allow these sessions to continue starting up.
# https://github.com/posit-dev/positron/issues/2070
# https://github.com/rstudio/renv/blob/5d0d52c395e569f7f24df4288d949cef95efca4e/inst/resources/activate.R#L85-L87
in_renv_autoloader <- function() {
  identical(getOption("renv.autoloader.running"), TRUE)
}

hook_readline <- function(prompt = "") {
  if (in_renv_autoloader()) {
    return("n")
  }

  if (!is_string(prompt)) {
    # Safety
    prompt <- ""
  }

  message <- paste0(collapse = "\n", c(
    "Can't call `readline()` within an `.Rprofile` or `.Rprofile.site` file.",
    sprintf("- `readline()` called with prompt '%s'.", prompt)
  ))

  stop(message, call. = FALSE)
}

hook_menu <- function(choices, graphics = FALSE, title = NULL) {
  if (!is.character(choices)) {
    # Safety
    choices <- ""
  }
  if (!is_string(title)) {
    # Safety (also replaces `NULL`, which is probably not common to see anyways)
    title <- ""
  }

  choices <- paste0(choices, collapse = ", ")

  message <- paste0(collapse = "\n", c(
    "Can't call `menu()` within an `.Rprofile` or `.Rprofile.site` file.",
    sprintf("- `menu()` called with title '%s' and choices '%s'.", title, choices)
  ))

  stop(message, call. = FALSE)
}
