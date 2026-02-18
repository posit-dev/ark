#
# calls_deparse.R
#
# Copyright (C) 2025 Posit Software, PBC. All rights reserved.
#
#

deparse_string <- function(x, cutoff = 500L) {
    paste_line(deparse(x, width.cutoff = cutoff))
}

as_label <- function(x) {
    # Remove arguments of call expressions
    if (is.call(x) && call_print_type(x) == "prefix") {
        x <- x[1]
    }

    # Retain only first line
    out <- deparse(x)[[1]]

    # And first 20 characters
    if (nchar(out) > 20) {
        out <- substr(out, 1, 20)
        out <- paste0(out, "...")
    }

    out
}

is_simple_call <- function(x) {
    call_print_type(x) == "prefix"
}

# From https://github.com/r-lib/rlang/blob/main/R/call.R
call_print_type <- function(call) {
    stopifnot(is.call(call))

    type <- call_print_fine_type(call)
    switch(
        type,
        call = "prefix",
        control = ,
        delim = ,
        subset = "special",
        type
    )
}

call_print_fine_type <- function(call) {
    stopifnot(is.call(call))

    op <- call_parse_type(call)
    if (op == "") {
        return("call")
    }

    switch(
        op,
        `+unary` = ,
        `-unary` = ,
        `~unary` = ,
        `?unary` = ,
        `!` = ,
        `!!` = ,
        `!!!` = "prefix",
        `function` = ,
        `while` = ,
        `for` = ,
        `repeat` = ,
        `if` = "control",
        `(` = ,
        `{{` = ,
        `{` = "delim",
        `[` = ,
        `[[` = "subset",
        # These operators always print in infix form even if they have
        # more arguments
        `<-` = ,
        `<<-` = ,
        `=` = ,
        `::` = ,
        `:::` = ,
        `$` = ,
        `@` = "infix",
        `+` = ,
        `-` = ,
        `?` = ,
        `~` = ,
        `:=` = ,
        `|` = ,
        `||` = ,
        `&` = ,
        `&&` = ,
        `>` = ,
        `>=` = ,
        `<` = ,
        `<=` = ,
        `==` = ,
        `!=` = ,
        `*` = ,
        `/` = ,
        `%%` = ,
        `special` = ,
        `:` = ,
        `^` = if (length(call) == 3) {
            "infix"
        } else {
            "call"
        }
    )
}

# Extracted from C implementation in src/internal/parse.c
call_parse_type <- function(call) {
    if (!is.call(call)) {
        return("")
    }

    head <- call[[1]]
    if (!is.symbol(head)) {
        return("")
    }

    # Check if unary by examining if there's only one argument after the head
    is_unary <- length(call) == 2

    # Control flow keywords
    if (identical(head, quote(`break`))) {
        return("break")
    }
    if (identical(head, quote(`next`))) {
        return("next")
    }
    if (identical(head, quote(`for`))) {
        return("for")
    }
    if (identical(head, quote(`while`))) {
        return("while")
    }
    if (identical(head, quote(`repeat`))) {
        return("repeat")
    }
    if (identical(head, quote(`if`))) {
        return("if")
    }
    if (identical(head, quote(`function`))) {
        return("function")
    }

    # Question mark (help operator)
    if (identical(head, quote(`?`))) {
        if (is_unary) {
            return("?unary")
        }
        return("?")
    }

    # Assignment operators
    if (identical(head, quote(`<-`))) {
        return("<-")
    }
    if (identical(head, quote(`<<-`))) {
        return("<<-")
    }
    if (identical(head, quote(`=`))) {
        return("=")
    }
    if (identical(head, quote(`:=`))) {
        return(":=")
    }

    # Comparison operators
    if (identical(head, quote(`<`))) {
        return("<")
    }
    if (identical(head, quote(`<=`))) {
        return("<=")
    }
    if (identical(head, quote(`>`))) {
        return(">")
    }
    if (identical(head, quote(`>=`))) {
        return(">=")
    }
    if (identical(head, quote(`==`))) {
        return("==")
    }
    if (identical(head, quote(`!=`))) {
        return("!=")
    }

    # Tilde (formula operator)
    if (identical(head, quote(`~`))) {
        if (is_unary) {
            return("~unary")
        }
        return("~")
    }

    # Logical operators
    if (identical(head, quote(`|`))) {
        return("|")
    }
    if (identical(head, quote(`||`))) {
        return("||")
    }
    if (identical(head, quote(`&`))) {
        return("&")
    }
    if (identical(head, quote(`&&`))) {
        return("&&")
    }

    # Bang operators (for negation, unquoting is unsupported)
    if (identical(head, quote(`!`))) {
        return("!")
    }

    # Arithmetic operators
    if (identical(head, quote(`+`))) {
        if (is_unary) {
            return("+unary")
        }
        return("+")
    }
    if (identical(head, quote(`-`))) {
        if (is_unary) {
            return("-unary")
        }
        return("-")
    }
    if (identical(head, quote(`*`))) {
        return("*")
    }
    if (identical(head, quote(`/`))) {
        return("/")
    }
    if (identical(head, quote(`^`))) {
        return("^")
    }

    # Modulo and special operators
    if (identical(head, quote(`%%`))) {
        return("%%")
    }

    # Check for special operators like %in%, %*%, etc.
    name <- as.character(head)
    if (
        substr(name, 1, 1) == "%" &&
            nchar(name) > 2 &&
            substr(name, nchar(name), nchar(name)) == "%"
    ) {
        return("special")
    }

    # Colon operators
    if (identical(head, quote(`:`))) {
        return(":")
    }
    if (identical(head, quote(`::`))) {
        return("::")
    }
    if (identical(head, quote(`:::`))) {
        return(":::")
    }

    # Access operators
    if (identical(head, quote(`$`))) {
        return("$")
    }
    if (identical(head, quote(`@`))) {
        return("@")
    }

    # Subsetting operators
    if (identical(head, quote(`[`))) {
        return("[")
    }
    if (identical(head, quote(`[[`))) {
        return("[[")
    }

    # Parentheses
    if (identical(head, quote(`(`))) {
        return("(")
    }

    # Braces and embrace
    if (identical(head, quote(`{`))) {
        # Check for embrace operator: {{x}}
        if (length(call) == 2) {
            cadr <- call[[2]]
            if (
                is.call(cadr) &&
                    length(cadr) == 2 &&
                    identical(cadr[[1]], quote(`{`)) &&
                    is.symbol(cadr[[2]])
            ) {
                return("{{")
            }
        }
        return("{")
    }

    ""
}

backtick <- function(x) {
    if (needs_backticks(x)) {
        paste0("`", x, "`")
    } else {
        x
    }
}

needs_backticks <- function(str) {
    stopifnot(is_string(str))

    n <- nchar(str)
    if (!n) {
        return(FALSE)
    }

    if (str %in% .reserved_words) {
        return(TRUE)
    }

    start <- substr(str, 1, 1)
    if (!grepl("[[:alpha:].]", start)) {
        return(TRUE)
    }

    if (n == 1) {
        return(FALSE)
    }

    remaining <- substr(str, 2, n)

    # .0 double literals
    if (start == "." && grepl("^[[:digit:]]", remaining)) {
        return(TRUE)
    }

    grepl("[^[:alnum:]_.]", remaining)
}

# From gram.y
.reserved_words <- c(
    "NULL",
    "NA",
    "TRUE",
    "FALSE",
    "Inf",
    "NaN",
    "NA_integer_",
    "NA_real_",
    "NA_character_",
    "NA_complex_",
    "function",
    "while",
    "repeat",
    "for",
    "if",
    "in",
    "else",
    "next",
    "break"
)
