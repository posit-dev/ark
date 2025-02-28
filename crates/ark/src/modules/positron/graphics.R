#
# graphics.R
#
# Copyright (C) 2022-2025 by Posit Software, PBC
#
#

# Set up "before new page" hooks
setHook("before.plot.new", action = "replace", function(...) {
    .ps.Call("ps_graphics_before_new_page", "before.plot.new")
})
setHook("before.grid.newpage", action = "replace", function(...) {
    .ps.Call("ps_graphics_before_new_page", "before.grid.newpage")
})

# A persistent list mapping plot `id`s to their display list recording.
# Used for replaying recordings under a new device or new width/height/resolution.
RECORDINGS <- list()

# Retrieves a recording by its `id`
#
# Returns `NULL` if no recording exists
getRecording <- function(id) {
    RECORDINGS[[id]]
}

addRecording <- function(id, recording) {
    RECORDINGS[[id]] <<- recording
}

# TODO: Use this when we get notified that we can remove a recording
# removeRecording <- function(id) {
#     RECORDINGS[[id]] <<- NULL
# }

plotRecordingRoot <- function() {
    root <- file.path(tempdir(), "positron-plot-recordings")
    ensure_directory(root)
    root
}

plotRecordingPath <- function(id) {
    root <- plotRecordingRoot()
    file <- paste0("recording-", id, ".png")
    file.path(root, file)
}

#' @export
.ps.graphics.createDevice <- function(name, type) {
    # Get path where non-recorded plots will be generated.
    root <- plotRecordingRoot()
    filename <- file.path(root, "current-plot.png")

    if (is.null(type)) {
        type <- defaultDeviceType()
    }

    # TODO: Is there any way to know the `pixel_ratio` here ahead of time?
    # We know and use it in `.ps.graphics.renderPlotFromRecording()`.
    res <- defaultResolutionInPixelsPerInch()

    # Create the graphics device.
    # TODO: Use 'ragg' if available?
    withCallingHandlers(
        grDevices::png(
            filename = filename,
            type = type,
            res = res
        ),
        warning = function(w) {
            stop("Error creating graphics device: ", conditionMessage(w))
        }
    )

    # Update the device name + description in the base environment.
    index <- grDevices::dev.cur()
    oldDevice <- .Devices[[index]]
    newDevice <- name

    # Copy device attributes. Usually, this is just the file path.
    attributes(newDevice) <- attributes(oldDevice)

    # Set other device properties.
    attr(newDevice, "type") <- type
    attr(newDevice, "res") <- res

    # Update the devices list.
    .Devices[[index]] <- newDevice

    # Replace bindings.
    env_bind_force(baseenv(), ".Devices", .Devices)
    env_bind_force(baseenv(), ".Device", newDevice)

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
.ps.graphics.recordPlot <- function(id) {
    # Create the plot recording
    recording <- grDevices::recordPlot()

    # Add the recording to the persistent list
    addRecording(id, recording)

    invisible(NULL)
}

#' @export
.ps.graphics.renderPlotFromRecording <- function(
    id,
    width,
    height,
    pixel_ratio,
    format
) {
    recording <- getRecording(id)
    recordingPath <- plotRecordingPath(id)

    if (is.null(recording)) {
        stop(sprintf(
            "Failed to render plot for plot `id` %s. Recording is missing.",
            id
        ))
    }

    # Replay the plot with the specified device.
    withDevice(recordingPath, width, height, pixel_ratio, format, {
        suppressWarnings(grDevices::replayPlot(recording))
    })

    # Return path to generated plot file.
    invisible(recordingPath)
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
withDevice <- function(
    path,
    width,
    height,
    pixel_ratio,
    format,
    expr
) {
    # Store handle to current device (i.e. us)
    old_dev <- grDevices::dev.cur()

    args <- finalizeDeviceDimensions(format, width, height, pixel_ratio)
    width <- args$width
    height <- args$height
    res <- args$res
    type <- args$type

    # Create a new graphics device.
    switch(
        format,
        "png" = grDevices::png(
            filename = path,
            width = width,
            height = height,
            res = res,
            type = type
        ),
        "svg" = grDevices::svg(
            filename = path,
            width = width,
            height = height,
        ),
        "pdf" = grDevices::pdf(
            file = path,
            width = width,
            height = height
        ),
        "jpeg" = grDevices::jpeg(
            filename = path,
            width = width,
            height = height,
            res = res,
            type = type
        ),
        "tiff" = grDevices::tiff(
            filename = path,
            width = width,
            height = height,
            res = res,
            type = type
        ),
        stop("Internal error: Unknown plot `format`.")
    )

    # Ensure we turn off the device on the way out, this:
    # - Commits the plot to disk
    # - Resets us back as being the current device
    on.exit(utils::capture.output({
        grDevices::dev.off()
        if (old_dev > 1) {
            grDevices::dev.set(old_dev)
        }
    }))

    expr
}

finalizeDeviceDimensions <- function(format, width, height, pixel_ratio) {
    if (format == "png" || format == "jpeg" || format == "tiff") {
        # These devices require `width` and `height` in pixels.
        # We already have them in pixels, so we just scale by the `pixel_ratio`.
        # `res` is nominal resolution specified in pixels-per-inch (ppi).
        return(list(
            type = defaultDeviceType(),
            res = defaultResolutionInPixelsPerInch() * pixel_ratio,
            width = width * pixel_ratio,
            height = height * pixel_ratio
        ))
    }

    if (format == "svg" || format == "pdf") {
        # These devices require `width` and `height` in inches.
        # We convert from pixels to inches here.
        # There is no `type` or `res` argument for these devices.
        return(list(
            type = NULL,
            res = NULL,
            width = width * pixel_ratio / defaultResolutionInPixelsPerInch(),
            height = height * pixel_ratio / defaultResolutionInPixelsPerInch()
        ))
    }

    stop("Internal error: Unknown plot `format`.")
}

defaultResolutionInPixelsPerInch <- function() {
    if (Sys.info()[["sysname"]] == "Darwin") {
        96L
    } else {
        72L
    }
}

defaultDeviceType <- function() {
    if (has_aqua()) {
        "quartz"
    } else if (has_cairo()) {
        "cairo"
    } else if (has_x11()) {
        "Xlib"
    } else {
        stop("This version of R wasn't built with plotting capabilities")
    }
}
