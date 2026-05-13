#
# html_widgets.R
#
# Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
#
#

#' @export
.ps.is_notebook <- function() {
    .ps.Call("ps_is_notebook")
}

#' @export
.ps.view_html_widget <- function(x, ...) {
    if (isTRUE(.ps.is_notebook())) {
        view_html_widget_inline(x)
        return(invisible(x))
    }

    view_html_widget_viewer(x)
    invisible(x)
}

# Notebook / background path: render the widget to a single self-contained
# HTML document, with each JS/CSS dependency inlined as a `data:` URI, then
# emit it as a `display_data` IOPub message. The notebook saves the payload
# verbatim, so it must not reference temp files on disk.
view_html_widget_inline <- function(x) {
    rendered <- htmltools::renderTags(x)
    html <- embed_tags(rendered)
    label <- widget_label(x)
    .ps.Call("ps_html_widget_emit", html, label)
}

# Console path: write the widget to a temp HTML file via htmltools, then hand
# the path to Positron's UI comm so it can be served by the Viewer pane.
# The temp directory survives for the life of the R session, so relative
# references from the HTML to its sibling `*_files/` directory resolve.
view_html_widget_viewer <- function(x) {
    rendered <- htmltools::as.tags(x, standalone = TRUE)
    tmp_file <- htmltools::html_print(rendered, viewer = NULL)

    # Guess whether this is a plot-like widget based on its sizing policy.
    destination <- if (isTRUE(x$sizingPolicy$knitr$figure)) "plot" else "viewer"

    # Derive the height of the viewer pane from the sizing policy of the widget.
    height <- .ps.validate.viewer.height(x$sizingPolicy$viewer$paneHeight)

    label <- widget_label(x)

    # Pass the widget to the viewer. Positron will assemble the final HTML
    # document from these components.
    .ps.Call("ps_html_viewer", tmp_file, label, height, destination)
}

# Build a self-contained `<!DOCTYPE html>` document from a `renderTags()`
# result. Each dependency is inlined as base64 `data:` URIs so that the
# returned string can be saved into a Jupyter notebook and reopened without
# any external file references.
embed_tags <- function(rendered) {
    deps <- filter_seen_deps(rendered$dependencies)
    dep_html <- vapply(deps, render_dep_inline, character(1))

    head_parts <- c(
        '<meta charset="utf-8"/>',
        dep_html,
        if (nzchar(rendered$head %||% "")) rendered$head else NULL
    )

    paste0(
        "<!DOCTYPE html>\n",
        "<html>\n",
        "<head>\n",
        paste(head_parts, collapse = "\n"),
        "\n</head>\n",
        "<body>\n",
        rendered$html,
        "\n</body>\n",
        "</html>\n"
    )
}

# Render one `htmlDependency` as the `<link>`/`<script>` block to embed in
# `<head>`. Local files (`src$file`) are base64-inlined; CDN-only deps
# (`src$href`) fall back to a remote reference, which is best-effort
# self-containment but at least keeps the widget functional online.
render_dep_inline <- function(dep) {
    file_base <- dep$src[["file"]]
    href_base <- dep$src[["href"]]

    parts <- character()

    # Stylesheets
    for (css in as_named_resource(dep$stylesheet)) {
        if (!is.null(file_base)) {
            parts <- c(parts, sprintf(
                '<link rel="stylesheet" href="%s"/>',
                file_to_data_uri(file.path(file_base, css), "text/css")
            ))
        } else if (!is.null(href_base)) {
            parts <- c(parts, sprintf(
                '<link rel="stylesheet" href="%s/%s"/>',
                href_base,
                css
            ))
        }
    }

    # Scripts
    for (js in as_named_resource(dep$script)) {
        if (!is.null(file_base)) {
            parts <- c(parts, sprintf(
                '<script src="%s"></script>',
                file_to_data_uri(file.path(file_base, js), "application/javascript")
            ))
        } else if (!is.null(href_base)) {
            parts <- c(parts, sprintf(
                '<script src="%s/%s"></script>',
                href_base,
                js
            ))
        }
    }

    # Inline <script>/<style> blocks the dep wants in <head>.
    if (length(dep$head) && nzchar(dep$head)) {
        parts <- c(parts, dep$head)
    }

    paste(parts, collapse = "\n")
}

# `htmlDependency()` allows `script`/`stylesheet` to be either a character
# vector or a list of named lists (with `src=` and other attributes for
# subresource integrity etc.). Normalize to a character vector of source
# paths; richer attributes are dropped on the floor for now.
as_named_resource <- function(x) {
    if (is.null(x)) {
        return(character())
    }
    if (is.character(x)) {
        return(x)
    }
    vapply(x, function(item) {
        if (is.character(item)) item else item[["src"]] %||% NA_character_
    }, character(1))
}

# Read a file and return a `data:<mime>;base64,...` URI. Used for inlining
# JS/CSS dependencies. The mime types we pass in are static; charset for CSS
# is set explicitly so non-ASCII glyphs in fonts.css etc. render correctly.
# (base64enc is a hard dependency of htmltools, so it's guaranteed to be
# available wherever this code path runs.)
file_to_data_uri <- function(path, mime) {
    bytes <- readBin(path, what = "raw", n = file.info(path)$size)
    encoded <- base64enc::base64encode(bytes)
    mime_with_charset <- if (identical(mime, "text/css")) {
        "text/css;charset=utf-8"
    } else {
        mime
    }
    paste0("data:", mime_with_charset, ";base64,", encoded)
}

# Per-session dedup: when enabled, each `htmlDependency` keyed by
# `name@version` is inlined once. Subsequent widgets in the same session that
# share a dep (e.g. two plotly figures both pulling in plotly.js) emit just
# their body markup and rely on the earlier cell's `<script>` having
# registered the library globally.
#
# This only works on frontends that render `text/html` outputs into a shared
# DOM (classic Jupyter, JupyterLab). Positron's notebook view isolates each
# cell's output, so a deduped second widget would find an empty global scope
# and render blank. Default off; opt in with
# `options(ark.html_widget.deduplicate = TRUE)` if you know your frontend
# shares scope across cells.
filter_seen_deps <- function(deps) {
    if (!isTRUE(getOption("ark.html_widget.deduplicate", FALSE))) {
        return(deps)
    }

    cache <- html_dep_cache()
    keep <- logical(length(deps))
    for (i in seq_along(deps)) {
        dep <- deps[[i]]
        key <- paste0(dep$name, "@", dep$version)
        if (is.null(cache[[key]])) {
            cache[[key]] <- TRUE
            keep[i] <- TRUE
        }
    }
    deps[keep]
}

html_dep_cache <- function() {
    if (is.null(the$html_dep_cache)) {
        the$html_dep_cache <- new.env(parent = emptyenv())
    }
    the$html_dep_cache
}

# Test hook: clear the per-session dedup cache so tests can assert dedup
# behavior independent of each other.
#' @export
.ps.html_widget_reset_deps <- function() {
    the$html_dep_cache <- NULL
    invisible(NULL)
}

# Derive a human-readable label for the `text/plain` fallback from the
# widget's class. Falls back to a generic label if the class is empty.
widget_label <- function(x) {
    label <- class(x)[1]
    if (length(label) && nzchar(label)) {
        paste(label, "HTML widget")
    } else {
        "R HTML widget"
    }
}

#' @export
.ps.viewer.addOverrides <- function() {
    add_s3_override("print.htmlwidget", .ps.view_html_widget)
}

#' @export
.ps.viewer.removeOverrides <- function() {
    remove_s3_override("print.htmlwidget")
}

# When the htmlwidgets package is loaded, inject/overlay our print method.
loadEvent <- packageEvent("htmlwidgets", "onLoad")
setHook(
    loadEvent,
    function(...) {
        .ps.viewer.addOverrides()
    },
    action = "append"
)

unloadEvent <- packageEvent("htmlwidgets", "onUnload")
setHook(
    unloadEvent,
    function(...) {
        .ps.viewer.removeOverrides()
    },
    action = "append"
)

# Validate the height argument for the viewer function; returns an
# integer or stops with an error.
.ps.validate.viewer.height <- function(height) {
    if (identical(height, "maximize")) {
        # The height of the viewer pane is set to -1 to maximize it.
        height <- -1L
    }
    if (!is.null(height) && (!is.numeric(height) || (length(height) != 1))) {
        stop(
            "Invalid height: ",
            height,
            "Must be a single element numeric vector or 'maximize'."
        )
    }
    if (is.null(height)) {
        # The height of the viewer pane is set to 0 to signal that
        # no specific height is requested.
        height <- 0L
    }
    as.integer(height)
}

# Derive a title for the viewer from the given file path
.ps.viewer.title <- function(path) {
    # Use the filename as the label, unless it's an index file, in which
    # case use the directory name.
    fname <- tolower(basename(path))
    if (identical(fname, "index.html") || identical(fname, "index.htm")) {
        fname <- basename(dirname(path))
    }

    # R HTML widgets get printed to temporary files starting with the name
    # "viewhtml". This makes an ugly label, so we give it a nicer one.
    if (startsWith(fname, "viewhtml")) {
        "R HTML widget"
    } else {
        fname
    }
}
