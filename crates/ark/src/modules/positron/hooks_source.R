initialize_hooks_source <- function() {
    node_poke_cdr(as.symbol(".ark_annotate_source"), .ark_annotate_source)

    # When:
    # - The input path is a file that has breakpoints
    # - No other arguments than `echo` (used by Positron) or `local` are provided
    # We opt into a code path where breakpoints are injected and the whole source is
    # wrapped in `{}` to allow stepping through it.
    rebind("base", "source", make_ark_source(base::source), namespace = TRUE)
}

make_ark_source <- function(original_source) {
    force(original_source)

    # Take all original arguments for e.g. completions
    function(
        file,
        local = FALSE,
        echo = verbose,
        print.eval = echo,
        exprs,
        spaced = use_file,
        verbose = getOption("verbose"),
        prompt.echo = getOption("prompt"),
        max.deparse.length = 150,
        width.cutoff = 60L,
        deparseCtrl = "showAttributes",
        chdir = FALSE,
        catch.aborts = FALSE,
        encoding = getOption("encoding"),
        continue.echo = getOption("continue"),
        skip.echo = 0,
        keep.source = getOption("keep.source"),
        ...
    ) {
        # Compute default argument for `spaced`. Must be defined before the
        # fallback calls.
        use_file <- missing(exprs)

        # DRY: Promise for calling `original_source` with all arguments.
        # Evaluated lazily only when needed for fallback paths.
        delayedAssign(
            "fall_back",
            original_source(
                file = file,
                local = local,
                echo = echo,
                print.eval = print.eval,
                exprs = exprs,
                spaced = spaced,
                verbose = verbose,
                prompt.echo = prompt.echo,
                max.deparse.length = max.deparse.length,
                width.cutoff = width.cutoff,
                deparseCtrl = deparseCtrl,
                chdir = chdir,
                catch.aborts = catch.aborts,
                encoding = encoding,
                continue.echo = continue.echo,
                skip.echo = skip.echo,
                keep.source = keep.source,
                ...
            )
        )

        # Fall back if hook is disabled
        if (!isTRUE(getOption("ark.source_hook", default = TRUE))) {
            return(fall_back)
        }

        call <- match.call()

        # Ignore `echo` and `local` arguments
        call$echo <- NULL
        call$local <- NULL

        # Fall back if `file` is not supplied or if any argument other than
        # `echo` or `local` is supplied
        if (is.null(call$file) || length(call[-1]) != 1) {
            return(fall_back)
        }

        uri <- path_to_file_uri(file)
        if (is.null(uri)) {
            return(fall_back)
        }

        env <- if (isTRUE(local)) {
            parent.frame()
        } else if (isFALSE(local)) {
            .GlobalEnv
        } else if (is.environment(local)) {
            local
        } else {
            return(fall_back)
        }

        text <- paste(
            readLines(uri, encoding = encoding, warn = FALSE),
            collapse = "\n"
        )
        annotated <- .ps.Call("ps_annotate_source", uri, text)

        # If NULL, no breakpoints exist for that URI, fall back
        if (is.null(annotated)) {
            return(fall_back)
        }

        log_trace(sprintf(
            "DAP: `source()` hook called with breakpoint injection for `uri`='%s'",
            uri
        ))

        parsed <- parse(text = annotated, keep.source = TRUE)

        if (length(parsed) != 1) {
            log_trace("`source()`: Expected a single `{}` expression")
        }

        # `eval()` loops over the expression vector, handling gracefully
        # unexpected lengths (0 or >1)
        eval(parsed, env)
    }
}

#' @export
.ark_annotate_source <- function(source, uri) {
    stopifnot(
        is_string(source),
        is_string(uri)
    )
    .ps.Call("ps_annotate_source", source, uri)
}
