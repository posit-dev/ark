# Extract source code from an installed R package using srcref metadata.
#
# Arguments (via commandArgs):
#   1. package - Package name
#   2. version - Expected package version
#
# Library paths have been set ahead of time via the `R_LIBS` environment
# variable.
#
# On success: prints the concatenated source lines (with `#line` directives) to stdout.
# On known failure: exits with code 2 (package not installed, version mismatch, or no srcrefs).

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

# Get the package namespace and find a function object.
# Functions will contain the srcrefs if srcrefs have been kept, and are what we
# can back out the full file structure from.
ns <- asNamespace(package)
exports <- getNamespaceExports(ns)

fn <- NULL
for (name in exports) {
    candidate <- get0(name, envir = ns, mode = "function")
    if (!is.null(candidate)) {
        fn <- candidate
        break
    }
}

if (is.null(fn)) {
    message(paste0("No functions found for package '", package, "'"))
    quit(status = 2L)
}

extract_lines <- function(fn) {
    srcref <- attr(fn, "srcref")
    if (is.null(srcref)) {
        return(NULL)
    }

    srcfile <- attr(srcref, "srcfile")
    if (is.null(srcfile)) {
        return(NULL)
    }

    original <- srcfile$original
    if (is.null(original)) {
        return(NULL)
    }

    lines <- original$lines
    if (is.null(lines)) {
        return(NULL)
    }

    lines
}

lines <- extract_lines(fn)

if (is.null(lines)) {
    message(paste0("No srcrefs found for package '", package, "'"))
    quit(status = 2L)
}

cat(lines, sep = "\n")
