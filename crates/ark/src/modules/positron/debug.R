#
# debug.R
#
# Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
#
#

debugger_stack_info <- function(
    context_call_text,
    context_last_start_line,
    context_srcref,
    fns,
    environments,
    calls
) {
    n <- length(fns)
    if (n != length(environments) || n != length(calls)) {
        message <- paste0(
            "`sys.function()`, `sys.frames()`, and `sys.calls()` didn't return consistent results. ",
            "There are %i functions, %i frames, and %i calls."
        )
        stop(sprintf(message, n, length(environments), length(calls)))
    }

    if (n == 0L) {
        return(list(frame_info_from_srcref(
            source_name = "<global>.R",
            frame_name = "<global>",
            srcref = context_srcref,
            environment = NULL
        )))
    }

    top_level_loc <- 1L
    top_level_call <- calls[[top_level_loc]]

    # Last function and environment go with the context, and will be used as needed.
    # Last call is the call that dropped us into the `context_fn`, and can be used
    # to generate an informative frame name.
    context_loc <- n
    context_fn <- fns[[context_loc]]
    context_environment <- environments[[context_loc]]
    context_frame_call <- calls[[context_loc]]

    # - Remove `top_level_loc` to make `intermediate_calls`, as top level call
    #   is handled elsewhere.
    # - Remove `context_loc` to make
    #   `intermediate_fns/environments/frame_calls`, as context versions of
    #   those are handled elsewhere.
    # - This actually aligns the `intermediate_calls` with the
    #   `intermediate_fns/environments/frame_calls` in a way that is useful to
    #   us when constructing frame information (i.e. we end up wanting the
    #   function, environment, and frame associated with the call you evaluate
    #   inside that function).
    intermediate_n <- n - 1L
    intermediate_calls <- calls[-top_level_loc]
    intermediate_fns <- fns[-context_loc]
    intermediate_environments <- environments[-context_loc]
    intermediate_frame_calls <- calls[-context_loc]

    # Now compute all of the pieces and put them together
    first_frame_info <- top_level_call_frame_info(top_level_call)
    intermediate_frame_infos <- intermediate_frame_infos(
        intermediate_n,
        intermediate_calls,
        intermediate_fns,
        intermediate_environments,
        intermediate_frame_calls
    )
    last_frame_info <- context_frame_info(
        context_srcref,
        context_fn,
        context_environment,
        context_call_text,
        context_frame_call,
        context_last_start_line
    )

    out <- c(
        list(first_frame_info),
        intermediate_frame_infos,
        list(last_frame_info)
    )

    out
}

top_level_call_frame_info <- function(x) {
    source_name <- paste0(as_label(x), ".R")

    srcref <- attr(x, "srcref", exact = TRUE)
    if (!is.null(srcref)) {
        out <- frame_info_from_srcref(
            source_name = source_name,
            frame_name = "<global>",
            srcref = srcref,
            environment = NULL
        )

        if (!is.null(out)) {
            return(out)
        }
    }

    # We return `0`s to avoid highlighting anything in the top level call.
    # We just want to show it in the editor, and that's really it.
    new_frame_info(
        source_name = source_name,
        frame_name = "<global>",
        file = NULL,
        contents = x,
        environment = NULL,
        start_line = 0L,
        start_column = 0L,
        end_line = 0L,
        end_column = 0L
    )
}

context_frame_info <- function(
    srcref,
    fn,
    environment,
    call_text,
    frame_call,
    last_start_line
) {
    frame_name <- as_label(frame_call)
    source_name <- paste0(frame_name, ".R")

    frame_info(
        source_name,
        frame_name,
        srcref,
        fn,
        environment,
        call_text,
        last_start_line
    )
}

intermediate_frame_infos <- function(n, calls, fns, environments, frame_calls) {
    srcrefs <- lapply(calls, function(call) {
        attr(call, "srcref", exact = TRUE)
    })
    call_texts <- lapply(calls, function(call) {
        call_lines <- call_deparse(call)
        call_text <- paste_line(call_lines)
        call_text
    })
    frame_names <- lapply(frame_calls, function(call) as_label(call))

    # Currently only tracked for the context frame, as that is where it is most useful,
    # since that is where the user is actively stepping.
    last_start_line <- NULL

    out <- vector("list", n)

    for (i in seq_len(n)) {
        srcref <- srcrefs[[i]]
        fn <- fns[[i]]
        environment <- environments[[i]]
        call_text <- call_texts[[i]]
        frame_name <- frame_names[[i]]

        out[[i]] <- frame_info(
            source_name = paste0(frame_name, ".R"),
            frame_name = frame_name,
            srcref = srcref,
            fn = fn,
            environment = environment,
            call_text = call_text,
            last_start_line = last_start_line
        )
    }

    out
}

frame_info <- function(
    source_name,
    frame_name,
    srcref,
    fn,
    environment,
    call_text,
    last_start_line
) {
    if (!is.null(srcref)) {
        # Prefer srcref if we have it
        out <- frame_info_from_srcref(
            source_name,
            frame_name,
            srcref,
            environment
        )

        if (!is.null(out)) {
            return(out)
        }
    }

    # Only deparse if `srcref` failed!
    fn_lines <- call_deparse(fn)
    fn_text <- paste_line(fn_lines)

    # Reparse early on, so even if we fail to find `call_text` or fail to reparse,
    # we pass a `fn_text` to `frame_info_unknown_range()` where we've consistently removed
    # any known non parseable objects in the text. This is particularly important right when
    # we step into a function without sources, where we aren't able to match against the
    # whole function body in the first step, but we are able to start matching on the next
    # step.
    info <- reparse_frame_text(fn_text, call_text)
    fn_expr <- info$fn_expr
    fn_text <- info$fn_text
    call_text <- info$call_text

    if (!is.null(fn_expr) && !is.null(call_text)) {
        # Fallback to matching against `call_text` if we have to and we have it and we were
        # able to successfully parse `fn_text`.
        out <- frame_info_from_function(
            source_name = source_name,
            frame_name = frame_name,
            environment = environment,
            fn_expr = fn_expr,
            fn_text = fn_text,
            call_text = call_text,
            last_start_line = last_start_line
        )

        if (!is.null(out)) {
            return(out)
        }
    }

    frame_info_unknown_range(
        source_name = source_name,
        frame_name = frame_name,
        file = NULL,
        contents = fn_text,
        environment = environment
    )
}

frame_info_from_srcref <- function(
    source_name,
    frame_name,
    srcref,
    environment
) {
    info <- srcref_info(srcref)
    if (is.null(info)) {
        return(NULL)
    }

    if (is_string(info$file)) {
        source_name <- basename(info$file)
    }

    new_frame_info(
        source_name = source_name,
        frame_name = frame_name,
        file = info$file,
        contents = info$content,
        environment = environment,
        start_line = info$range$start_line,
        start_column = info$range$start_column,
        end_line = info$range$end_line,
        end_column = info$range$end_column
    )
}

frame_info_from_function <- function(
    source_name,
    frame_name,
    environment,
    fn_expr,
    fn_text,
    call_text,
    last_start_line
) {
    # Immediately after we step into a function, R spits out `debug: { entire-body }`,
    # which doesn't show up in our source references so we don't find it and end up
    # returning `0`s for the locations. But this is ok, all the user has to do is step to
    # the first line of the function, and then we start recognizing expressions again.
    range <- locate_call(fn_expr, call_text, last_start_line)

    if (is.null(range)) {
        return(NULL)
    }

    new_frame_info(
        source_name = source_name,
        frame_name = frame_name,
        file = NULL,
        contents = fn_text,
        environment = environment,
        start_line = range$start_line,
        start_column = range$start_column,
        end_line = range$end_line,
        end_column = range$end_column
    )
}

reparse_frame_text <- function(fn_text, call_text) {
    fn_expr <- parse_function_text(fn_text)

    if (is.null(fn_expr)) {
        # Likely due to a non parseable object.
        # In these cases we try to strip out known non parseable object descriptions that
        # come from `deparse()` and we try to parse again. We return the updated `fn_text`
        # so we can display text in the editor that matches the text we performed the matching
        # against.
        fn_text <- replace_non_parseable(fn_text)

        if (!is.null(call_text)) {
            # Also update `call_text` for consistency during matching if we have it.
            call_text <- replace_non_parseable(call_text)
        }

        # Could still return `NULL`, caller deals with that
        fn_expr <- parse_function_text(fn_text)
    }

    list(
        fn_expr = fn_expr,
        fn_text = fn_text,
        call_text = call_text
    )
}

frame_info_unknown_range <- function(
    source_name,
    frame_name,
    file,
    contents,
    environment
) {
    new_frame_info(
        source_name = source_name,
        frame_name = frame_name,
        file = file,
        contents = contents,
        environment = environment,
        start_line = 0L,
        start_column = 0L,
        end_line = 0L,
        end_column = 0L
    )
}

new_frame_info <- function(
    source_name,
    frame_name,
    file,
    contents,
    environment,
    start_line,
    start_column,
    end_line,
    end_column
) {
    list(
        source_name = source_name,
        frame_name = frame_name,
        file = file,
        contents = contents,
        environment = environment,
        start_line = start_line,
        start_column = start_column,
        end_line = end_line,
        end_column = end_column
    )
}

#' @param fn_expr A function expression returned from `parse_function_text()`, which
#'   reparsed the function text while keeping source references.
#' @param call_text A single string containing the text of a call to look for
#'   in the function, with lines split by `\n`.
#' @param last_start_line Either `NULL` if the last start line is unknown, or a single
#'   integer specifying the last `start_line` for this frame. Only actually used for the
#'   current context frame. Used to more precisely handle matching ambiguities by using
#'   this as a minimum start line to filter with. Definitely not a perfect heuristic,
#'   but works decently well. Would be too complicated and not very useful to try and
#'   track this for other frames than the current context frame that the user is actually
#'   actively stepping through.
#'
#' @returns A range created by `srcref_to_range()` that points to a location in `fn_expr`.
locate_call <- function(fn_expr, call_text, last_start_line) {
    info <- extract_source_references(fn_expr)
    fn_text <- info$text

    fn_srcref <- info$srcref
    fn_ranges <- lapply(fn_srcref, srcref_to_range)

    # Drop whitespace characters everywhere
    fn_text <- gsub("\\s", "", fn_text)
    call_text <- gsub("\\s", "", call_text)

    # Drop newline characters everywhere
    fn_text <- gsub("\\n", "", fn_text)
    call_text <- gsub("\\n", "", call_text)

    # Try for an exact match
    matches <- which(call_text == fn_text)

    if (length(matches) == 0L) {
        # If we failed to find an exact match, allow for partial matching within the text.
        # This is useful in the case of things like `if (cond) expr` where `expr` isn't
        # surrounded by `{` so we don't get parse information for it, but R still shows
        # `debug: expr` and we end up capturing that. We will end up highlighting the whole
        # `if` expression even when we are on `expr`, but it is better than jumping to the
        # beginning of the function.
        matches <- locate_partial_matches(call_text, fn_text, fn_ranges)
    }

    n_matches <- length(matches)

    if (n_matches == 0L) {
        # If still no matches, just abort mission
        return(NULL)
    } else if (n_matches == 1L) {
        match <- matches
    } else {
        # With multiple matches, try to disambiguate if we can using the last start line
        if (is.null(last_start_line)) {
            # Best we can do
            match <- matches[[1L]]
        } else {
            match <- filter_with_last_start_line(
                matches,
                fn_ranges,
                last_start_line
            )
        }
    }

    range <- fn_ranges[[match]]

    range
}

filter_with_last_start_line <- function(matches, fn_ranges, last_start_line) {
    fn_ranges <- fn_ranges[matches]
    fn_start_lines <- vapply(
        fn_ranges,
        FUN.VALUE = integer(1),
        function(range) range$start_line
    )

    # Sort by increasing start line
    order <- order(fn_start_lines)
    fn_start_lines <- fn_start_lines[order]
    matches <- matches[order]

    # Find the start lines past the `last_start_line`.
    # Using `>` rather than `>=` to try and force the highlighted line to always move forward
    # even if the same expression is repeated multiple times in a row.
    candidate <- which(fn_start_lines > last_start_line)

    # If for some reason we don't have any hits, choose the line closest to the `last_start_line`.
    if (length(candidate) == 0L) {
        candidate <- which.min(last_start_line - fn_start_lines)
    }

    # If there are multiple start line candidates, we choose the first as that is closest
    # to the `last_start_line` and is likely where the user is
    if (length(candidate) > 1L) {
        candidate <- candidate[[1L]]
    }

    matches[[candidate]]
}

locate_partial_matches <- function(call_text, fn_text, fn_ranges) {
    matches <- grep(call_text, fn_text, fixed = TRUE)

    # Filter the `matches` down to only the leaf nodes that don't contain any other nodes
    fn_ranges <- fn_ranges[matches]

    loc <- locate_leaves(fn_ranges)
    matches <- matches[loc]

    matches
}

#' Extracts out srcref nodes that don't contain any other nodes.
#' These are the only nodes we want to partial match against.
locate_leaves <- function(xs) {
    out <- integer()

    for (i in seq_along(xs)) {
        candidate <- xs[[i]]
        leaf <- TRUE

        for (j in seq_along(xs)) {
            if (i == j) {
                next
            }

            node <- xs[[j]]

            if (contains(candidate, node)) {
                leaf <- FALSE
                break
            }
        }

        if (leaf) {
            out <- c(out, i)
        }
    }

    out
}

contains <- function(x, y) {
    contains_lower(
        x$start_line,
        x$start_column,
        y$start_line,
        y$start_column
    ) &&
        contains_upper(x$end_line, x$end_column, y$end_line, y$end_column)
}

contains_lower <- function(x_line, x_column, y_line, y_column) {
    if (x_line < y_line) {
        TRUE
    } else if (x_line > y_line) {
        FALSE
    } else {
        # Equal lines is the only time we compare columns
        x_column <= y_column
    }
}

contains_upper <- function(x_line, x_column, y_line, y_column) {
    contains_lower(y_line, y_column, x_line, x_column)
}

#' Iterate through a function parsed with `keep.source = TRUE`
#' and recursively extract the source references for each possible cursor location.
#'
#' For example, with:
#'
#' ```
#' function(x) {
#'   1 + 1
#'
#'   if (x > 5) {
#'     2 + 2
#'   }
#'
#'   3
#' }
#' ```
#'
#' You'd get source reference information for:
#' - `1 + 1`
#' - `if (x > 5) { 2 + 2 }`
#' - `2 + 2`
#' - `3`
extract_source_references <- function(x) {
    srcref <- extract_source_references_recurse(x)

    # Convert srcref to text for matching
    text <- vapply(srcref, FUN.VALUE = character(1), function(x) {
        x <- as.character(x)
        paste0(x, collapse = "\n")
    })

    # Drop simple `{` srcrefs, we never stop there
    brace <- text == "{"
    text <- text[!brace]
    srcref <- srcref[!brace]

    list(text = text, srcref = srcref)
}

extract_source_references_recurse <- function(x) {
    out <- list()

    if (!is.call(x)) {
        return(out)
    }

    # Contribute this level's srcrefs
    if (inherits(x, "{")) {
        srcrefs <- attr(x, "srcref", exact = TRUE)

        if (is.list(srcrefs)) {
            out <- c(out, srcrefs)
        }
    }

    # Contribute children srcrefs
    for (i in seq_along(x)) {
        child <- x[[i]]

        if (missing(child)) {
            # Seen in `data.table::[.data.table`, for example, where they do
            # `"tail" = ,` in a switch statement
            next
        }

        extra <- extract_source_references_recurse(child)
        out <- c(out, extra)
    }

    out
}

#' Reparse a function with `keep.source = TRUE`
#'
#' @param x A single string containing the text of a function, with lines
#'   split by `\n`.
parse_function_text <- function(x) {
    x <- tryCatch(
        parse(text = x, keep.source = TRUE),
        error = function(cnd) NULL
    )

    if (is.null(x)) {
        return(NULL)
    }

    if (length(x) == 0L) {
        return(NULL)
    }

    # Returns an `expression(x)` object because there could be multiple functions,
    # but we only supply 1
    x <- x[[1]]

    if (!is.call(x)) {
        return(NULL)
    }

    x
}

replace_non_parseable <- function(x) {
    infos <- non_parseable_pattern_infos()

    for (info in infos) {
        pattern <- info$pattern
        replacement <- info$replacement
        fixed <- info$fixed

        x <- gsub(
            pattern = pattern,
            replacement = replacement,
            x = x,
            fixed = fixed
        )
    }

    x
}

# Hand crafted list collected by finding locations where `deparse()` sets
# `sourceable = FALSE`, and looking at the text that it inserts when that is the case
# https://github.com/wch/r-source/blob/2bbece03085f9227ed18726e0d0faab3d4d70262/src/main/deparse.c#L945-L946
#
# We replace the non parseable code with something that can parse, but purposefully looks
# suspicious. It is fairly hard to create one of these inside a function to begin with,
# but this will do it:
#
# ```
# options(keep.source = FALSE)
# fn <- rlang::inject(function() {
#   1 + 1
#   !!new.env()
#   "<environment>"
#   !!unclass(xml2::read_xml("<foo><bar /></foo>"))$node
# })
# ```
non_parseable_pattern_infos <- function() {
    list(
        non_parseable_pattern_info("<S4 object of class .*>", "...S4..."),
        non_parseable_pattern_info("<promise: .*>", "...PROMISE..."),
        non_parseable_pattern_info("<pointer: .*>", "...POINTER..."),
        non_parseable_fixed_info("<environment>", "...ENVIRONMENT..."),
        non_parseable_fixed_info("<bytecode>", "...BYTECODE..."),
        non_parseable_fixed_info("<weak reference>", "...WEAK_REFERENCE..."),
        non_parseable_fixed_info("<object>", "...OBJECT..."),
        # We see this one in `call_text` captured from `debug: <call>`,
        # not in `deparse()` directly. In the `fn_text` this shows up as `<environment>` and
        # we want to match that, so that's what we replace with.
        non_parseable_pattern_info("<environment: .*>", "...ENVIRONMENT...")
    )
}
non_parseable_pattern_info <- function(pattern, replacement) {
    list(pattern = pattern, replacement = replacement, fixed = FALSE)
}
non_parseable_fixed_info <- function(pattern, replacement) {
    list(pattern = pattern, replacement = replacement, fixed = TRUE)
}

is_breakpoint_enabled <- function(uri, id) {
    .ps.Call("ps_is_breakpoint_enabled", uri, id)
}

# Injected breakpoint. This receives a `browser()` call in the `expr` argument.
# The argument if forced if the breakpoint is enabled. Since `expr` is promised
# in the calling frame environment, that environment is marked by R as being
# debugged (with `SET_RDEBUG`), allowing to step through it. We're stopped in
# the wrong frame (`.ark_breakpoint()`'s) but the console automatically steps to
# the next expression whenever it detects that the current function (retrieved
# with `sys.function()`) inherits from `ark_breakpoint`.
#' @export
.ark_breakpoint <- structure(
    function(expr, uri, id) {
        # Force `browser()` call only if breakpoint is enabled
        if (!is_breakpoint_enabled(uri, id)) {
            return()
        }
        expr
    },
    class = "ark_breakpoint"
)

# Wrapper for expressions that should be auto-stepped over in the debugger. The
# debugger detects this by checking if R emitted a `debug at` line containing
# `.ark_auto_step` and automatically steps past it.
#' @export
.ark_auto_step <- function(expr) {
    expr
}

# Verify breakpoints in a line range. Called after each top-level expression in
# `source()`.
.ark_verify_breakpoints_range <- function(uri, start_line, end_line) {
    .ps.Call("ps_verify_breakpoints_range", uri, start_line, end_line)
}

debug_initialize <- function() {
    # Store `.ark_breakpoint` and friends in base namespace so they're maximally
    # reachable. We might want to do that for all symbols exported from the
    # Ark/Positron namespace.
    node_poke_cdr(as.symbol(".ark_annotate_source"), .ark_annotate_source)
    node_poke_cdr(as.symbol(".ark_auto_step"), .ark_auto_step)
    node_poke_cdr(as.symbol(".ark_breakpoint"), .ark_breakpoint)
    node_poke_cdr(
        as.symbol(".ark_verify_breakpoints_range"),
        .ark_verify_breakpoints_range
    )
}
