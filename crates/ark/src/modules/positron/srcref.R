# Populate source references right there and then instead of at idle time. Used
# for `View()` when called from top-level. Also useful for debugging from R. Be
# careful with this one because it produces session-wide side effects that
# mutate source references for all functions in the given namespace. This could
# invalidate reasonable assumptions made by currently running code.
ns_populate_srcref <- function(ns_name) {
    loadNamespace(ns_name)
    .ps.Call("ps_ns_populate_srcref", ns_name)
}

ns_populate_srcref_without_vdoc_insertion <- function(ns_name) {
    loadNamespace(ns_name)
    .ps.Call("ps_ns_populate_srcref_without_vdoc_insertion", ns_name)
}

fn_populate_srcref_without_vdoc_insertion <- function(fn) {
    fn_env <- topenv(environment(fn))
    if (!isNamespace(fn_env)) {
        return(NULL)
    }

    pkg <- getNamespaceName(fn_env)
    ns_populate_srcref_without_vdoc_insertion(pkg)
}


# Called from Rust
reparse_with_srcref <- function(x, name, uri, line) {
    if (!is.function(x)) {
        stop("Must be a function")
    }

    line_directive <- paste("#line", line)
    text <- c(line_directive, deparse(x))
    srcfile <- srcfilecopy(uri, text)

    # This may fail if not reparsable
    expr <- parse(
        text = text,
        keep.source = TRUE,
        srcfile = srcfile
    )

    # Evaluate in namespace to materialise the function
    out <- eval(expr, environment(x))

    # Now check that body and formals were losslessly reparsed. In theory
    # `identical()` should ignore srcrefs but it seems buggy with nested ones,
    # so we zap them beforehand.
    if (!identical(zap_srcref(x), zap_srcref(out))) {
        stop("Can't reparse function losslessly")
    }

    # Remove line directive
    text <- text[-1]

    # Make sure fake source includes function name
    name <- deparse(name, backtick = TRUE)
    text[[1]] <- paste(name, "<-", text[[1]])

    list(obj = out, text = text)
}

zap_srcref <- function(x) {
    .ps.Call("ark_zap_srcref", x)
}

new_ark_debug <- function(fn) {
    # Signature of `debug()` and `debugonce()`:
    # function(fun, text = "", condition = NULL, signature = NULL)

    body(fn) <- bquote({
        local({
            if (!.ps.internal(do_resource_namespaces(default = TRUE))) {
                return() # from local()
            }

            pkgs <- loadedNamespaces()

            # Give priority to the namespace of the debugged function
            env <- topenv(environment(fun))
            if (isNamespace(env)) {
                pkgs <- unique(c(getNamespaceName(env), pkgs))
            }

            # Enable namespace resourcing for all future loaded namespaces and
            # resource already loaded namespaces so we get virtual documents for
            # step-debugging.
            options(ark.resource_namespaces = TRUE)
            .ps.internal(resource_namespaces(pkgs))
        })

        .(body(fn))
    })

    fn
}

do_resource_namespaces <- function(default) {
    getOption("ark.resource_namespaces", default = default)
}

resource_namespaces <- function(pkgs) {
    .ps.Call("ps_resource_namespaces", pkgs)
}

srcref_info <- function(srcref) {
    srcfile <- attr(srcref, "srcfile")
    if (is.null(srcfile)) {
        return(NULL)
    }

    # If the file name is missing but there is a `srcref`, then we can try to use
    # the `lines` to reconstruct a fake source file that `srcref` can point into.
    # This is used when debugging user functions that are entered directly into the console,
    # and for functions parsed with `parse(text = <text>, keep.source = TRUE)`.
    file <- srcfile$filename
    lines <- srcfile$lines

    if (!identical(file, "") && !identical(file, "<text>")) {
        if (!is_ark_uri(file)) {
            # TODO: Handle absolute paths by using `wd`
            file <- normalizePath(file, mustWork = FALSE)
        }
        content <- NULL
    } else if (!is.null(lines)) {
        file <- NULL
        content <- paste0(lines, collapse = "\n")
    } else {
        return(NULL)
    }

    range <- srcref_to_range(srcref)

    list(
        file = file,
        content = content,
        range = range
    )
}

srcref_to_range <- function(x) {
    n <- length(x)

    # The first and third fields are sensitive to #line directives if they exist,
    # which we want to honour in order to jump to original files
    # rather than generated files.
    loc_start_line <- 1L
    loc_end_line <- 3L

    # We need the `column` value rather than the `byte` value, so we
    # can index into a character. However the srcref documentation
    # allows a 4 elements vector when the bytes and column values are
    # the same. We account for this here.
    if (n >= 6) {
        loc_start_column <- 5L
        loc_end_column <- 6L
    } else {
        loc_start_column <- 2L
        loc_end_column <- 4L
    }

    list(
        start_line = x[[loc_start_line]],
        start_column = x[[loc_start_column]],
        end_line = x[[loc_end_line]],
        end_column = x[[loc_end_column]]
    )
}
