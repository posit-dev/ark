# Adapted from https://github.com/r-lib/rlang/blob/main/R/standalone-cli.R

CLI_SUPPORT_HYPERLINK <- "2.2.0"
CLI_SUPPORT_HYPERLINK_PARAMS <- "3.1.1"

has_cli <- function(min_version = NULL) {
    if (is.null(the$cli_version)) {
        return(FALSE)
    }

    if (is_string(min_version) && the$cli_version < min_version) {
        return(FALSE)
    }

    TRUE
}

ansi_info <- function() col_blue(symbol_info())
symbol_info <- function() if (has_cli()) cli::symbol$info else "i"
col_blue <- function(x) if (has_cli()) cli::col_blue(x) else x

style_hyperlink <- function(text, url, params = NULL) {
    if (is.null(params)) {
        if (has_cli(CLI_SUPPORT_HYPERLINK)) {
            cli::style_hyperlink(text, url)
        } else {
            text
        }
    } else {
        if (has_cli(CLI_SUPPORT_HYPERLINK_PARAMS)) {
            cli::style_hyperlink(text, url, params = params)
        } else {
            text
        }
    }
}
