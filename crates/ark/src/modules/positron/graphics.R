#
# graphics.R
#
# Copyright (C) 2022-2025 by Posit Software, PBC
#
#

# Set up "before plot new" hooks. This is our cue for
# saving up the state of a plot before it gets wiped out.
setHook("before.plot.new", action = "replace", function(...) {
    .ps.Call("ps_graphics_before_plot_new", "before.plot.new")
})
setHook("before.grid.newpage", action = "replace", function(...) {
    .ps.Call("ps_graphics_before_plot_new", "before.grid.newpage")
})

# A persistent list mapping plot `id`s to their display list recording.
# Used for replaying recordings under a new device or new width/height/resolution.
RECORDINGS <- list()

# Retrieves a recording by its `id`
#
# Returns `NULL` if no recording exists
get_recording <- function(id) {
    RECORDINGS[[id]]
}

add_recording <- function(id, recording) {
    RECORDINGS[[id]] <<- recording
}

# Called when a plot comm is closed by the frontend
remove_recording <- function(id) {
    RECORDINGS[[id]] <<- NULL
}

render_directory <- function() {
    directory <- file.path(tempdir(), "positron-plot-renderings")
    ensure_directory(directory)
    directory
}

render_path <- function(id, format) {
    directory <- render_directory()
    file <- paste0("render-", id, ".", format)
    file.path(directory, file)
}

#' @export
.ps.graphics.create_device <- function() {
    name <- "Ark Graphics Device"

    # Create the graphics device that we are going to shadow.
    # Creating a graphics device mutates global state, we don't need to capture
    # the return value.
    device_shadow()

    # Update the device name + description in the base environment.
    index <- grDevices::dev.cur()
    old_device <- .Devices[[index]]
    new_device <- name

    # Copy device attributes. Usually, this is just the file path.
    attributes(new_device) <- attributes(old_device)

    # Update the devices list.
    .Devices[[index]] <- new_device

    # Replace bindings.
    env_bind_force(baseenv(), ".Devices", .Devices)
    env_bind_force(baseenv(), ".Device", new_device)

    # Also set ourselves as a known interactive device.
    # Used by `dev.interactive()`, which is used in `stats:::plot.lm()`
    # to determine if `devAskNewPage(TRUE)` should be set to prompt before
    # each new plot is drawn.
    grDevices::deviceIsInteractive(name)
}

# Create a recording of the current plot.
#
# This saves the plot's display list, so it can be used to re-render plots as
# necessary.
#' @export
.ps.graphics.record_plot <- function(id) {
    # Create the plot recording
    recording <- grDevices::recordPlot()

    # Add the recording to the persistent list
    add_recording(id, recording)

    invisible(NULL)
}

#' @export
.ps.graphics.render_plot_from_recording <- function(
    id,
    width,
    height,
    pixel_ratio,
    format
) {
    path <- render_path(id, format)
    recording <- get_recording(id)

    if (is.null(recording)) {
        stop(sprintf(
            "Failed to render plot for plot `id` %s. Recording is missing.",
            id
        ))
    }

    # Replay the plot with the specified device.
    with_graphics_device(path, width, height, pixel_ratio, format, {
        suppressWarnings(grDevices::replayPlot(recording))
    })

    # Return path to generated plot file.
    invisible(path)
}

#' Run an expression with the specificed device activated.
#'
#' The device is guaranteed to close after the expression has run.
#'
#' @param path The file path to render output to.
#' @param width The plot width, in pixels.
#' @param height The plot height, in pixels.
#' @param pixel_ratio The device pixel ratio (e.g. 1 for standard displays, 2
#'   for retina displays)
#' @param format The output format (and therefore graphics device) to use.
#'   One of: `"png"`, `"svg"`, `"pdf"`, `"jpeg"`, or `"tiff"`.
with_graphics_device <- function(
    path,
    width,
    height,
    pixel_ratio,
    format,
    expr
) {
    # Store handle to current device (i.e. us)
    old_dev <- grDevices::dev.cur()

    args <- finalize_device_arguments(format, width, height, pixel_ratio)
    width <- args$width
    height <- args$height
    res <- args$res

    # Create a new graphics device.
    switch(
        format,
        "png" = device_png(
            filename = path,
            width = width,
            height = height,
            res = res
        ),
        "svg" = device_svg(
            filename = path,
            width = width,
            height = height
        ),
        "pdf" = device_pdf(
            filename = path,
            width = width,
            height = height
        ),
        "jpeg" = device_jpeg(
            filename = path,
            width = width,
            height = height,
            res = res
        ),
        "tiff" = device_tiff(
            filename = path,
            width = width,
            height = height,
            res = res
        ),
        stop("Internal error: Unknown plot `format`.")
    )

    # Ensure we turn off the device on the way out, this:
    # - Commits the plot to disk
    # - Resets us back as being the current device
    defer(utils::capture.output({
        grDevices::dev.off()
        if (old_dev > 1) {
            grDevices::dev.set(old_dev)
        }
    }))

    expr
}

use_ragg <- local({
    # Only check global option once per session
    delayedAssign("use_ragg", init_use_ragg())
    function() use_ragg
})

use_svglite <- local({
    # Only check global option once per session
    delayedAssign("use_svglite", init_use_svglite())
    function() use_svglite
})

init_use_ragg <- function() {
    option <- getOption("ark.ragg", default = TRUE)

    if (!isTRUE(option)) {
        # Bail on any non-`TRUE` option value
        return(FALSE)
    }

    if (!.ps.is_installed("ragg", minimum_version = "1.4.0")) {
        # Need support for `agg_record()`
        return(FALSE)
    }

    TRUE
}

init_use_svglite <- function() {
    option <- getOption("ark.svglite", default = TRUE)

    if (!isTRUE(option)) {
        # Bail on any non-`TRUE` option value
        return(FALSE)
    }

    if (!.ps.is_installed("svglite")) {
        return(FALSE)
    }

    TRUE
}

#' Create a device to shadow
#'
#' For both ragg and png, we are hopeful that providing a `res` of the default
#' resolution should not affect the render time results much (since we are just
#' writing display list instructions), even if at render time we can actually
#' support a `pixel_ratio` of 2x the default resolution. We simply don't know
#' the pixel ratio at this point.
device_shadow <- function() {
    if (use_ragg()) {
        # For the shadow ragg device, we use a special device that only captures
        # the display list, it doesn't actually do any rendering!
        ragg::agg_record(
            res = default_resolution_in_pixels_per_inch()
        )
    } else {
        # For the shadow png device, we need a dummy file to write to,
        # even though we never look at it. We only utilize the png device for
        # the action of recording the display list.
        directory <- render_directory()
        filename <- file.path(directory, "dummy-plot.png")

        withCallingHandlers(
            grDevices::png(
                filename = filename,
                type = default_device_type(),
                res = default_resolution_in_pixels_per_inch()
            ),
            warning = function(w) {
                stop("Error creating graphics device: ", conditionMessage(w))
            }
        )
    }
}

device_png <- function(filename, width, height, res) {
    if (use_ragg()) {
        ragg::agg_png(
            filename = filename,
            width = width,
            height = height,
            res = res
        )
    } else {
        grDevices::png(
            filename = filename,
            width = width,
            height = height,
            res = res,
            type = default_device_type()
        )
    }
}

device_svg <- function(filename, width, height) {
    if (use_svglite()) {
        svglite::svglite(
            filename = filename,
            width = width,
            height = height
        )
    } else {
        grDevices::svg(
            filename = filename,
            width = width,
            height = height
        )
    }
}

device_pdf <- function(filename, width, height) {
    grDevices::pdf(
        file = filename,
        width = width,
        height = height
    )
}

device_jpeg <- function(filename, width, height, res) {
    if (use_ragg()) {
        ragg::agg_jpeg(
            filename = filename,
            width = width,
            height = height,
            res = res
        )
    } else {
        grDevices::jpeg(
            filename = filename,
            width = width,
            height = height,
            res = res,
            type = default_device_type()
        )
    }
}

device_tiff <- function(filename, width, height, res) {
    if (use_ragg()) {
        ragg::agg_tiff(
            filename = filename,
            width = width,
            height = height,
            res = res
        )
    } else {
        grDevices::tiff(
            filename = filename,
            width = width,
            height = height,
            res = res,
            type = default_device_type()
        )
    }
}

finalize_device_arguments <- function(format, width, height, pixel_ratio) {
    if (format == "png" || format == "jpeg" || format == "tiff") {
        # These devices require `width` and `height` in pixels, which is what
        # they are provided in already. For pixel based devices, all relevant
        # values are upscaled by `pixel_ratio`.
        #
        # `res` is nominal resolution specified in pixels-per-inch (ppi).
        return(list(
            res = default_resolution_in_pixels_per_inch() * pixel_ratio,
            width = width * pixel_ratio,
            height = height * pixel_ratio
        ))
    }

    if (format == "svg" || format == "pdf") {
        # These devices require `width` and `height` in inches, but they are
        # provided to us in pixels, so we have to perform a conversion here.
        # For vector based devices, providing the size in inches implicitly
        # tells the device the relative size to use for things like text,
        # since that is the absolute unit (pts are based on inches).
        #
        # Thomas says the math for `width` and `height` here are correct, i.e.
        # we don't also multiply `default_resolution_in_pixels_per_inch()` by
        # `pixel_ratio` like we do above, which would have made it cancel out of
        # the equation below.
        #
        # There is no `res` argument for these devices.
        return(list(
            res = NULL,
            width = width *
                pixel_ratio /
                default_resolution_in_pixels_per_inch(),
            height = height *
                pixel_ratio /
                default_resolution_in_pixels_per_inch()
        ))
    }

    stop("Internal error: Unknown plot `format`.")
}

#' Default OS resolution in PPI (pixels per inch)
#'
#' Thomas thinks these are "more correct than any other numbers." Specifically,
#' macOS uses 96 DPI for its internal scaling, but this is user definable on
#' Windows.
#'
#' This corresponds to a scaling factor that tries to make things that appear
#' "on screen" be as close to the size in which they are actually printed at,
#' which has always been tricky.
default_resolution_in_pixels_per_inch <- function() {
    if (Sys.info()[["sysname"]] == "Darwin") {
        96L
    } else {
        72L
    }
}

#' Determines the default device `type` for png, jpeg, and tiff
#'
#' Only applicable when ragg is not in use
default_device_type <- function() {
    switch(
        system_os(),
        macos = default_device_type_macos(),
        windows = default_device_type_windows(),
        linux = default_device_type_linux(),
        # Treat `other` as linux
        other = default_device_type_linux()
    )
}

#' On MacOS, we prefer Quartz
#'
#' At one point we considered preferring Cairo, but `capabilities("cairo")`
#' isn't a reliable signal of whether or not you have Cairo support, because
#' surprisingly you also need xquartz installed as well, i.e. with `brew install
#' --cask xquartz`. Confusingly you don't need that to use the `"quartz"` type.
#' https://github.com/posit-dev/positron/issues/913
#' https://github.com/posit-dev/positron/issues/2919
#'
#' From https://cran.r-project.org/doc/manuals/r-release/R-admin.html#Installing-R-under-macOS-1:
#' "Various parts of the build require XQuartz to be installed...This is also
#' needed for some builds of the cairographics-based devices...such as
#' png(type = "cairo") and svg()..."
#'
#' To avoid this issue for Mac users, we don't even consider Cairo in our
#' fallback path. It seems unlikely we'd ever get past `"quartz"` as the
#' fallback anyways, it's probably installed on all Macs.
default_device_type_macos <- function() {
    if (has_aqua()) {
        "quartz"
    } else if (has_x11()) {
        "Xlib"
    } else {
        stop_no_plotting_capabilities()
    }
}

#' On Windows, we prefer Cairo
#'
#' According to Thomas, this is much preferred over the default on Windows,
#' which uses the Windows GDI. We don't even offer that as a fallback.
default_device_type_windows <- function() {
    if (has_cairo()) {
        "cairo"
    } else {
        stop_no_plotting_capabilities()
    }
}

#' On Linux, we prefer Cairo
#'
#' This is the default there, and we have no reason to move away from it.
default_device_type_linux <- function() {
    if (has_cairo()) {
        "cairo"
    } else if (has_x11()) {
        "Xlib"
    } else {
        stop_no_plotting_capabilities()
    }
}

stop_no_plotting_capabilities <- function() {
    stop("This version of R wasn't built with plotting capabilities")
}
