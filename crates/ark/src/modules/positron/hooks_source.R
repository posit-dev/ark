initialize_hooks_source <- function() {
    node_poke_cdr(as.symbol(".ark_annotate_source"), .ark_annotate_source)

    # When:
    # - The input path is a file that has breakpoints
    # - No other arguments than `echo` (used by Positron) or `local` are provided
    # We opt into a code path where breakpoints are injected and the whole source is
    # wrapped in `{}` to allow stepping through it. Note that `echo` has no
    # effect in the injection code path.
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

        # Capture environment early if local evaluation is requested.
        # This is necessary if we have to fallback when `local = TRUE`.
        if (isTRUE(local)) {
            local <- parent.frame()
        }

        args <- alist(
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

        # Remove arguments that are not yet supported
        if (getRversion() <= "4.4.0") {
            args$catch.aborts <- NULL
        }

        # Try to resolve the file URI early so we can attribute plots to this
        # source file. This is best-effort; if it fails we proceed without attribution.
        source_uri <- tryCatch(path_to_file_uri(file), error = function(e) NULL)

        # Push source context for plot attribution (if we have a URI).
        # The on.exit ensures we always pop, even if source() errors.
        # Use tryCatch so that source() still works if the native function
        # is not yet available (e.g. during development with mismatched builds).
        if (!is.null(source_uri)) {
            pushed <- tryCatch(
                { .ps.Call("ps_graphics_push_source_context", source_uri); TRUE },
                error = function(e) FALSE
            )
            if (pushed) {
                on.exit(tryCatch(.ps.Call("ps_graphics_pop_source_context"), error = function(e) NULL), add = TRUE)
            }
        }

        # DRY: Promise for calling `original_source` with all arguments.
        # Evaluated lazily only when needed for fallback paths.
        eval(bquote(
            delayedAssign(
                "fall_back",
                original_source(..(args))
            ),
            splice = TRUE
        ))

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

        # We already resolved the URI above; reuse it here
        uri <- source_uri
        if (is.null(uri)) {
            return(fall_back)
        }

        env <- if (isTRUE(local)) {
            stop(
                "Internal error: `local = TRUE` should have been converted to an environment above."
            )
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
        annotated <- .ark_annotate_source(text, uri, with_visible = TRUE)

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
            log_trace("`source()`: Expected a single `list()[[1]]` expression")
        }

        # `eval()` loops over the expression vector, handling gracefully
        # unexpected lengths (0 or >1). The annotated code is wrapped in
        # `withVisible()` so the result already has the right structure.
        invisible(eval(parsed, env))
    }
}

#' @export
.ark_annotate_source <- function(source, uri, with_visible = FALSE) {
    stopifnot(
        is_string(source),
        is_string(uri),
        is_bool(with_visible)
    )
    .ps.Call("ps_annotate_source", source, uri, with_visible)
}
