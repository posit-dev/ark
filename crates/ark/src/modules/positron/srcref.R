# For debugging from R
ns_populate_srcref <- function(ns_name) {
    loadNamespace(ns_name)
    .ps.Call("ps_ns_populate_srcref", ns_name)
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
    # Signature of `debug()`:
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
            .ps.internal(resource_loaded_namespaces(pkgs))
        })

        .(body(fn))
    })

    fn
}

do_resource_namespaces <- function(default) {
    getOption("ark.resource_namespaces", default = default)
}

resource_loaded_namespaces <- function(pkgs) {
    .ps.Call("ps_resource_loaded_namespaces", pkgs)
}
