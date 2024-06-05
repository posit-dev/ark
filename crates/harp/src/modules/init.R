try_catch_handler <- function(cnd) {
    # Save backtrace in error value
    calls <- sys.calls()

    # Remove handling context
    n <- length(calls)
    if (n > 3) {
        calls <- calls[-seq(n - 3, n)]
    }

    trace <- format_traceback(calls, rust_like = TRUE)

    message <- conditionMessage(cnd)
    class <- class(cnd)
    trace <- paste(trace, collapse = '\n')

    call <- conditionCall(cnd)
    if (!is.null(call)) {
        call <- paste(deparse(call), collapse = "\n")
    }

    list(message, call, class, trace)
}

#' @param traceback A list of calls.
#' @param rev Whether to list older calls first.
format_traceback <- function(traceback = list(), rust_like = FALSE) {
    n <- length(traceback)

    # TODO: This implementation prints the traceback in the same ordering
    # as rlang, i.e. with call 1 on the stack being the first thing you
    # see. `traceback()` prints in the reverse order, so users may want a
    # way to reverse both our display ordering and rlang's (requiring a
    # new `rlang::format()` argument).

    # Collect source location, if there is any
    srcrefs <- lapply(traceback, function(call) attr(call, "srcref"))
    srcrefs <- vapply(srcrefs, src_loc, FUN.VALUE = character(1))
    has_srcref <- nchar(srcrefs) != 0L
    srcrefs[has_srcref] <- vec_paste0(" at ", srcrefs[has_srcref])

    # Converts from a list of quoted calls to a list of deparsed calls.
    # Respects global options `"traceback.max.lines"` and `"deparse.max.lines"`!
    traceback <- .traceback(traceback)

    # Line prefix sequence
    seq <- seq_len(n)

    if (rust_like) {
        # Rust backtraces have younger frames first and count from 0
        traceback <- rev(traceback)
        seq <- seq - 1L
    }

    # Prepend the stack number to each deparsed call, padding multiline calls as needed,
    # and then collapse multiline calls into one line
    prefixes <- vec_paste0(seq, ". ")
    prefixes <- format(prefixes, justify = "right")

    traceback <- mapply(prepend_prefix, traceback, prefixes, SIMPLIFY = FALSE)
    traceback <- lapply(traceback, function(lines) paste0(lines, collapse = "\n"))
    traceback <- as.character(traceback)

    paste0(traceback, srcrefs)
}

prepend_prefix <- function(lines, prefix) {
    n_lines <- length(lines)

    if (n_lines == 0L) {
        return(lines)
    }

    # First line gets the prefix
    line <- lines[[1L]]
    line <- vec_paste0(prefix, line)

    # Other lines are padded with whitespace as needed
    padding <- strrep(" ", times = nchar(prefix))

    lines <- lines[-1L]
    lines <- vec_paste0(padding, lines)

    lines <- c(line, lines)

    lines
}

src_loc <- function(srcref) {
    # Adapted from `rlang:::src_loc()`
    if (is.null(srcref)) {
        return("")
    }

    srcfile <- attr(srcref, "srcfile")
    if (is.null(srcfile)) {
        return("")
    }

    # May be:
    # - An actual file path
    # - `""` for user defined functions in the console
    # - `"<text>"` for `parse()`d functions
    # We only try and display the source location for file paths
    file <- srcfile$filename
    if (identical(file, "") || identical(file, "<text>")) {
        return("")
    }

    file_trimmed <- path_trim_prefix(file, 3L)

    first_line <- srcref[[1L]]
    first_column <- srcref[[5L]]

    # TODO: We could generate file hyperlinks here like `rlang:::src_loc()`
    paste0(file_trimmed, ":", first_line, ":", first_column)
}

path_trim_prefix <- function(path, n) {
    # `rlang:::path_trim_prefix()`
    split <- strsplit(path, "/")[[1]]
    n_split <- length(split)

    if (n_split <= n) {
        path
    } else {
        paste(split[seq(n_split - n + 1, n_split)], collapse = "/")
    }
}

vec_paste0 <- function(..., collapse = NULL) {
    # Like `paste0()`, but avoids `paste0("prefix:", character())`
    # resulting in `"prefix:"` and instead recycles to size 0.
    # Assumes that inputs with size >0 would validly recycle to size 0.
    args <- list(...)

    if (any(lengths(args) == 0L)) {
        character()
    } else {
        args <- c(args, list(collapse = collapse))
        do.call(paste0, args)
    }
}
