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

# A persistent environment mapping plot `id`s to their display list recording.
# Used for replaying recordings under a new device or new width/height/resolution.
RECORDINGS <- new.env()

# Retrieves a recording by its `id`
#
# Returns `NULL` if no recording exists
getRecording <- function(id) {
    RECORDINGS[[id]]
}

addRecording <- function(id, recording) {
    RECORDINGS[[id]] <- recording
}

# TODO: Use this when we get notified that we can remove a recording
# removeRecording <- function(id) {
#     remove(list = id, envir = RECORDINGS)
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
.ps.graphics.createDevice <- function(name, type, res) {
    # Get path where non-recorded plots will be generated.
    root <- plotRecordingRoot()
    filename <- file.path(root, "current-plot.png")

    if (is.null(type)) {
        type <- defaultDeviceType()
    }

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

    # Add the recording to the persistent environment
    addRecording(id, recording)

    invisible(NULL)
}

#' @export
.ps.graphics.renderPlotFromRecording <- function(
    id,
    width,
    height,
    dpr,
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

    # Get device attributes to be passed along.
    type <- defaultDeviceType()
    res <- defaultResolution * dpr
    width <- width * dpr
    height <- height * dpr

    # Replay the plot with the specified device.
    withDevice(recordingPath, format, width, height, res, type, {
        suppressWarnings(grDevices::replayPlot(recording))
    })

    # Return path to generated plot file.
    invisible(recordingPath)
}

defaultResolution <- if (Sys.info()[["sysname"]] == "Darwin") {
    96L
} else {
    72L
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

# Run an expression with the specificed device activated.
# The device is guaranteed to close after the expression has run.
withDevice <- function(
    filepath,
    format,
    width,
    height,
    res,
    type,
    expr
) {
    # Store handle to current device (i.e. us)
    old_dev <- grDevices::dev.cur()

    # Width and height are in inches and use 72 DPI to create the requested
    # size in pixels
    dpi <- 72

    # Create a new graphics device.
    switch(
        format,
        "png" = grDevices::png(
            filename = filepath,
            width = width,
            height = height,
            res = res,
            type = type
        ),
        "svg" = grDevices::svg(
            filename = filepath,
            width = (width / dpi),
            height = (height / dpi),
        ),
        "pdf" = grDevices::pdf(
            file = filepath,
            width = (width / dpi),
            height = (height / dpi)
        ),
        "jpeg" = grDevices::jpeg(
            filename = filepath,
            width = width,
            height = height,
            res = res,
            type = type
        ),
        "tiff" = grDevices::tiff(
            filename = filepath,
            width = width,
            height = height,
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
