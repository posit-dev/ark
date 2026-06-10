#
# html_widgets.R
#
# Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
#
#

#' @export
.ps.view_html_widget <- function(x, ...) {
    if (session_mode() == "console") {
        # Interactive console: hand a temp HTML file to Positron's Viewer pane.
        view_html_widget_viewer(x)
    } else {
        # Notebook / background: emit self-contained HTML inline so it survives
        # notebook save/reload.
        view_html_widget_inline(x)
    }
    invisible(x)
}

# Notebook / background path: render the widget to a single self-contained
# HTML document, with each JS/CSS dependency inlined into a `<script>`/`<style>`
# block, then emit it as a `display_data` IOPub message. The notebook saves the
# payload verbatim, so it must not reference temp files on disk.
view_html_widget_inline <- function(x) {
    rendered <- htmltools::renderTags(x)
    html <- embed_tags(rendered)
    label <- widget_label(x)
    .ps.Call("ps_html_display_data", html, label)
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
# result. Each dependency's JS/CSS is inlined directly into `<script>`/`<style>`
# blocks so that the returned string can be saved into a Jupyter notebook and
# reopened without any external file references.
embed_tags <- function(rendered) {
    deps <- filter_seen_deps(rendered$dependencies)
    dep_html <- vapply(deps, render_dep_inline, character(1))

    # `renderTags()` yields "" (not NULL) when there is no `<head>` content;
    # normalize so it doesn't become a stray blank line. An existing NULL is
    # already fine and passes through `c()` untouched.
    if (identical(rendered$head, "")) {
        rendered$head <- NULL
    }

    head_parts <- c(
        '<meta charset="utf-8"/>',
        AMD_GUARD_OPEN,
        dep_html,
        rendered$head
    )

    paste0(
        "<!DOCTYPE html>\n",
        "<html>\n",
        "<head>\n",
        paste(head_parts, collapse = "\n"),
        "\n</head>\n",
        "<body>\n",
        rendered$html,
        "\n",
        AMD_GUARD_CLOSE,
        "\n",
        HTMLWIDGETS_RENDER,
        "\n</body>\n",
        "</html>\n"
    )
}

# htmlwidget JS dependencies are UMD bundles. When a global AMD `define` is
# present -- as it is inside Positron's and VS Code's output webviews -- these
# bundles take the AMD branch and register with the loader instead of attaching
# to the window. Leaflet, for instance, then never sets `window.L`, and the
# next dependency throws (`Cannot read properties of undefined (reading
# 'Proj')`) so the widget renders blank. We bracket the dependency and widget
# scripts to remove `define` for their (synchronous) duration, forcing the
# browser-global code path, then restore it. This mirrors the guard IRkernel
# uses for the same reason.
AMD_GUARD_OPEN <- "<script>window.__ark_define__ = window.define; window.define = undefined;</script>"
AMD_GUARD_CLOSE <- "<script>window.define = window.__ark_define__; try { delete window.__ark_define__; } catch (e) {}</script>"

# htmlwidgets initializes its widgets from the page's `DOMContentLoaded` event.
# When our output is inserted into an already-loaded document -- e.g. a notebook
# output webview that rehydrates the output's scripts long after the page itself
# loaded -- that event has already fired, so the widget would never render (it
# stays blank with no error). Trigger a render explicitly. `staticRender()` is
# idempotent (it skips elements already marked `html-widget-static-bound`), so
# in a freshly parsed document the later `DOMContentLoaded` render is a no-op.
HTMLWIDGETS_RENDER <- "<script>if (window.HTMLWidgets && window.HTMLWidgets.staticRender) { window.HTMLWidgets.staticRender(); }</script>"

# An `htmlDependency` (from htmltools) describes the front-end assets a widget
# needs: a name and version, a source location (either `src$file` on disk or a
# `src$href` URL), and `script`/`stylesheet` files given relative to that
# source. `renderTags()` returns these alongside the widget's HTML.
#
# Render one such dependency as the `<style>`/`<script>` block to embed in
# `<head>`. Local files (`src$file`) are inlined directly; CDN-only deps
# (`src$href`) fall back to a remote reference, which is best-effort
# self-containment but at least keeps the widget functional online.
render_dep_inline <- function(dep) {
    file_base <- dep$src[["file"]]
    href_base <- dep$src[["href"]]

    parts <- character()

    # Stylesheets
    for (css in as_named_resource(dep$stylesheet)) {
        if (!is.null(file_base)) {
            parts <- c(
                parts,
                sprintf(
                    "<style>\n%s\n</style>",
                    read_dep_file(file.path(file_base, css), "style")
                )
            )
        } else if (!is.null(href_base)) {
            parts <- c(
                parts,
                sprintf(
                    '<link rel="stylesheet" href="%s/%s"/>',
                    href_base,
                    css
                )
            )
        } else {
            # Neither a local file nor a URL to point at — there's nothing we
            # can reference, so the stylesheet is intentionally dropped.
        }
    }

    # Scripts
    for (js in as_named_resource(dep$script)) {
        if (!is.null(file_base)) {
            parts <- c(
                parts,
                sprintf(
                    "<script>\n%s\n</script>",
                    read_dep_file(file.path(file_base, js), "script")
                )
            )
        } else if (!is.null(href_base)) {
            parts <- c(
                parts,
                sprintf(
                    '<script src="%s/%s"></script>',
                    href_base,
                    js
                )
            )
        } else {
            # As above: no local file and no URL, so the script is dropped.
        }
    }

    # Inline <script>/<style> blocks the dep wants in <head>.
    if (length(dep$head) == 1L && nzchar(dep$head)) {
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
    vapply(
        x,
        function(item) {
            if (is.character(item)) item else item[["src"]] %||% NA_character_
        },
        character(1)
    )
}

# Read a JS/CSS dependency file and return its text, ready to drop straight
# into a `<script>`/`<style>` block. We inline the literal source rather than a
# base64 `data:` URI because `data:`-`src` scripts are loaded asynchronously by
# some notebook renderers (e.g. VS Code's, which rehydrates output scripts via
# `domEval`); that breaks load ordering and defeats the surrounding AMD guard,
# whereas inline scripts always run synchronously in source order. `tag` is
# "script" or "style" so we can neutralize any closing tag the content happens
# to contain.
read_dep_file <- function(path, tag) {
    bytes <- readBin(path, what = "raw", n = file.info(path)$size)
    text <- rawToChar(bytes)
    Encoding(text) <- "UTF-8"

    # Inline content may legitimately contain "</script>" or "</style>" inside
    # a string or comment, which would close the element early. Break the match
    # without changing the value the JS/CSS engine sees (`<\/script>` is the
    # same string as `</script>`).
    gsub(paste0("</", tag), paste0("<\\/", tag), text, fixed = TRUE)
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
# behavior independently. Reached from tests via
# `.ps.internal(html_widget_reset_deps())`.
html_widget_reset_deps <- function() {
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
