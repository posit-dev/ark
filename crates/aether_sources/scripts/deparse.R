# Deparse all functions from an installed R package's namespace.
#
# Arguments (via commandArgs):
#   1. package - Package name
#   2. version - Expected package version
#
# Library paths have been set ahead of time via the `R_LIBS` environment
# variable.
#
# On success: prints the deparsed namespace contents to stdout.
# On known failure: exits with code 2 (package not installed, version mismatch, or nothing to deparse).

# ------------------------------------------------------------------------------
# Helpers

replace_non_parseable <- function(x) {
    infos <- non_parseable_pattern_infos()

    for (info in infos) {
        x <- gsub(
            pattern = info$pattern,
            replacement = info$replacement,
            x = x,
            fixed = info$fixed,
            perl = !info$fixed
        )
    }

    x
}

non_parseable_pattern_infos <- function() {
    list(
        non_parseable_pattern_info("<S4 object of class .*?>", "...S4..."),
        non_parseable_pattern_info("<promise: .*?>", "...PROMISE..."),
        non_parseable_pattern_info("<pointer: .*?>", "...POINTER..."),
        non_parseable_fixed_info("<environment>", "...ENVIRONMENT..."),
        non_parseable_fixed_info("<bytecode>", "...BYTECODE..."),
        non_parseable_fixed_info("<weak reference>", "...WEAK_REFERENCE..."),
        non_parseable_fixed_info("<object>", "...OBJECT..."),
        non_parseable_pattern_info("<environment: .*?>", "...ENVIRONMENT...")
    )
}

non_parseable_pattern_info <- function(pattern, replacement) {
    list(pattern = pattern, replacement = replacement, fixed = FALSE)
}

non_parseable_fixed_info <- function(pattern, replacement) {
    list(pattern = pattern, replacement = replacement, fixed = TRUE)
}

# ------------------------------------------------------------------------------
# Main

args <- commandArgs(trailingOnly = TRUE)

if (length(args) != 2L) {
    message("'package' and 'version' are required arguments.")
    quit(status = 2L)
}

package <- args[[1L]]
version <- args[[2L]]

# Check if the package is installed
path <- find.package(package, quiet = TRUE)
if (length(path) == 0L) {
    message(paste0("Package '", package, "' is not installed"))
    quit(status = 2L)
}

# Check the installed version matches the requested version
installed_version <- as.character(packageVersion(package))
if (installed_version != version) {
    message(paste0(
        "Version mismatch for '",
        package,
        "': ",
        "installed ",
        installed_version,
        ", requested ",
        version
    ))
    quit(status = 2L)
}

ns <- asNamespace(package)
names <- ls(ns, all.names = TRUE)

lines_for_name <- vector("list", length = length(names))

for (i in seq_along(names)) {
    name <- names[[i]]

    if (bindingIsActive(name, ns)) {
        # We handle these specially to avoid any funny business
        next
    }

    fn <- get0(name, envir = ns, mode = "function")

    if (is.null(fn)) {
        # Binding wasn't a function
        next
    }

    lines <- tryCatch(
        deparse(fn),
        error = function(e) NULL
    )

    if (is.null(lines) || length(lines) == 0L) {
        next
    }

    # Make sure fake source includes function name.
    # Backtick non syntactic names (Like `[.method`).
    name <- deparse(as.symbol(name), backtick = TRUE)
    lines[[1L]] <- paste(name, "<-", lines[[1L]])

    lines_for_name[[i]] <- paste(lines, collapse = "\n")
}

# Drop `NULL`s where the name was invalid
lines_for_name <- lines_for_name[
    !vapply(lines_for_name, is.null, FUN.VALUE = logical(1))
]

if (length(lines_for_name) == 0L) {
    message(paste0("No deparsable objects found for package '", package, "'"))
    quit(status = 2L)
}

output <- unlist(lines_for_name)
output <- paste(output, collapse = "\n\n")
output <- replace_non_parseable(output)

cat(output)
