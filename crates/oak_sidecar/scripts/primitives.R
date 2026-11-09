# Write R stubs for the primitive functions of base R.
#
# Primitives are implemented in C, so base's R sources don't define them. Each
# stub gives a primitive the formals `args()` reports, with a `NULL` body.
# Primitives without formals (language constructs such as `if` and operators
# such as `[`) are bound to their `.Primitive()` call instead.
#
# The single command-line argument is the expected version of base.
# Print the stub file to stdout on success. Exit with code 1 if the argument
# is missing or the installed base version differs.

args <- commandArgs(trailingOnly = TRUE)

if (length(args) != 1L) {
    message("'version' is a required argument.")
    quit(status = 1L)
}

# Compare normalized package versions rather than their spelling.
version <- as.character(package_version(args[[1L]]))
installed_version <- as.character(packageVersion("base"))
if (installed_version != version) {
    message(paste0(
        "Version mismatch for 'base': installed ",
        installed_version,
        ", requested ",
        version
    ))
    quit(status = 1L)
}

quote_name <- function(name) {
    deparse(as.name(name), backtick = TRUE)
}

format_formal <- function(name, value) {
    if (identical(value, quote(expr = ))) {
        return(quote_name(name))
    }

    # Preserve newlines inside defaults. Joining deparsed lines with spaces
    # can merge separate expressions in a braced default.
    value <- paste(deparse(value, width.cutoff = 500L), collapse = "\n")
    paste0(quote_name(name), " = ", value)
}

format_stub <- function(name, fn) {
    formals_fn <- args(fn)

    if (is.null(formals_fn)) {
        return(paste0(quote_name(name), " <- .Primitive(", deparse(name), ")"))
    }

    formals <- formals(formals_fn)
    formatted <- vapply(
        seq_along(formals),
        function(i) format_formal(names(formals)[[i]], formals[[i]]),
        character(1)
    )

    paste0(
        quote_name(name),
        " <- function(",
        paste(formatted, collapse = ", "),
        ") NULL"
    )
}

# Locale-independent ordering keeps stub output reproducible for a given R build.
names <- sort(ls(baseenv(), all.names = TRUE), method = "radix")
is_primitive <- vapply(
    names,
    function(name) is.primitive(get(name, envir = baseenv())),
    logical(1)
)
names <- names[is_primitive]

stubs <- vapply(
    names,
    function(name) format_stub(name, get(name, envir = baseenv())),
    character(1),
    USE.NAMES = FALSE
)

header <- c(
    paste0(
        "# Generated from the primitive functions of this R build, ",
        R.version.string,
        "."
    ),
    "#",
    "# Each stub only describes the formals of a primitive, as `args()` reports",
    "# them. This is not base R's implementation of these functions, which is",
    "# written in C. Primitives without formals are bound to their `.Primitive()`",
    "# call.",
    ""
)

cat(c(header, stubs), sep = "\n")
