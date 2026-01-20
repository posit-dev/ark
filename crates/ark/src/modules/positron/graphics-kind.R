#
# graphics-kind.R
#
# Copyright (C) 2026 by Posit Software, PBC
#
#

#' Detect the kind of plot from a recording
#'
#' Uses multiple strategies to determine plot type:
#' 1. Check .Last.value for high-level plot objects (ggplot2, lattice)
#' 2. Check recording's display list for base graphics patterns
#' 3. Fall back to generic "plot"
#'
#' @param id The plot ID
#' @return A string describing the plot kind
#' @export
.ps.graphics.detect_plot_kind <- function(id) {
    # Strategy 1: Check .Last.value for recognizable plot objects
    # This works for ggplot2, lattice, and some other packages
    value <- tryCatch(
        get(".Last.value", envir = globalenv()),
        error = function(e) NULL
    )

    if (!is.null(value)) {
        kind <- detect_kind_from_value(value)
        if (!is.null(kind)) {
            return(kind)
        }
    }

    # Strategy 2: Check the recording itself
    recording <- get_recording(id)
    if (!is.null(recording)) {
        # recordPlot() stores display list in first element
        dl <- recording[[1]]
        if (length(dl) > 0) {
            kind <- detect_kind_from_display_list(dl)
            if (!is.null(kind)) {
                return(kind)
            }
        }
    }

    # Default fallback
    "plot"
}

# Detect plot kind from .Last.value
# Returns plot kind string or NULL
detect_kind_from_value <- function(value) {
    # ggplot2
    if (inherits(value, "ggplot")) {
        return(detect_ggplot_kind(value))
    }

    # lattice
    if (inherits(value, "trellis")) {
        # Extract lattice plot type from call
        call_fn <- as.character(value$call[[1]])
        kind_map <- c(
            "xyplot" = "scatter plot",
            "bwplot" = "box plot",
            "histogram" = "histogram",
            "densityplot" = "density plot",
            "barchart" = "bar chart",
            "dotplot" = "dot plot",
            "levelplot" = "heatmap",
            "contourplot" = "contour plot",
            "cloud" = "3D scatter",
            "wireframe" = "3D surface"
        )
        if (call_fn %in% names(kind_map)) {
            return(paste0("lattice ", kind_map[call_fn]))
        }
        return("lattice")
    }

    # Base R objects that have class
    if (inherits(value, "histogram")) {
        return("histogram")
    }
    if (inherits(value, "density")) {
        return("density")
    }
    if (inherits(value, "hclust")) {
        return("dendrogram")
    }
    if (inherits(value, "acf")) {
        return("autocorrelation")
    }

    NULL
}

# Detect ggplot2 plot kind from geom layers
# Returns plot kind string
detect_ggplot_kind <- function(gg) {
    if (length(gg$layers) == 0) {
        return("ggplot2")
    }

    # Get the first layer's geom class
    geom_class <- class(gg$layers[[1]]$geom)[1]
    geom_name <- tolower(gsub("^Geom", "", geom_class))

    kind_map <- c(
        "point" = "scatter plot",
        "line" = "line chart",
        "bar" = "bar chart",
        "col" = "bar chart",
        "histogram" = "histogram",
        "boxplot" = "box plot",
        "violin" = "violin plot",
        "density" = "density plot",
        "area" = "area chart",
        "tile" = "heatmap",
        "raster" = "raster",
        "contour" = "contour plot",
        "smooth" = "smoothed line",
        "text" = "text",
        "label" = "labels",
        "path" = "path",
        "polygon" = "polygon",
        "ribbon" = "ribbon",
        "segment" = "segments",
        "abline" = "reference lines",
        "hline" = "horizontal lines",
        "vline" = "vertical lines"
    )

    if (geom_name %in% names(kind_map)) {
        return(paste0("ggplot2 ", kind_map[geom_name]))
    }

    "ggplot2"
}

#' Retrieve plot metadata by display_id
#'
#' @param id The plot's display_id
#' @return A named list with fields: name, kind, execution_id, code.
#'   Returns NULL if no metadata is found for the given ID.
#' @export
.ps.graphics.get_metadata <- function(id) {
    .ps.Call("ps_graphics_get_metadata", id)
}

# Detect plot kind from display list (base graphics)
# Returns plot kind string or NULL
detect_kind_from_display_list <- function(dl) {
    # Display list entries are lists where first element is the C function name
    call_names <- vapply(dl, function(x) {
        if (is.list(x) && length(x) > 0) {
            name <- x[[1]]
            if (is.character(name)) name else ""
        } else {
            ""
        }
    }, character(1))

    # Base graphics C functions to plot types
    if (any(call_names == "C_plotHist")) return("histogram")
    if (any(call_names == "C_image")) return("image")
    if (any(call_names == "C_contour")) return("contour")
    if (any(call_names == "C_persp")) return("3D surface")
    if (any(call_names == "C_filledcontour")) return("filled contour")

    # Check for grid graphics (ggplot2, lattice)
    if (any(grepl("^L_", call_names))) {
        return("grid")
    }

    # Check for base graphics
    if (any(grepl("^C_", call_names))) {
        return("base")
    }

    NULL
}
