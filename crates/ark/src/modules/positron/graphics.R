#
# graphics.R
#
# Copyright (C) 2022-2024 by Posit Software, PBC
#
#

# Set up plot hooks.
setHook("before.plot.new", action = "replace", function(...) {
    .ps.Call("ps_graphics_event", "before.plot.new")
})

setHook("before.grid.newpage", action = "replace", function(...) {
    .ps.Call("ps_graphics_event", "before.grid.newpage")
})

default_device_type <- function() {
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

renderWithPlotDevice <- function(filepath, format, width, height, res, type) {
    # width and height are in inches and use 72 DPI to create the requested size in pixels
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
}

#' @export
.ps.graphics.defaultResolution <- if (Sys.info()[["sysname"]] == "Darwin") {
    96L
} else {
    72L
}

#' @export
.ps.graphics.plotSnapshotRoot <- function(...) {
    file.path(tempdir(), "positron-snapshots", ...)
}

#' @export
.ps.graphics.plotSnapshotPath <- function(id) {
    root <- .ps.graphics.plotSnapshotRoot(id)
    ensure_directory(root)
    file.path(root, "snapshot.rds")
}

#' @export
.ps.graphics.plotOutputPath <- function(id) {
    root <- .ps.graphics.plotSnapshotRoot(id)
    ensure_directory(root)
    file.path(root, "snapshot.png")
}

#' @export
.ps.graphics.createDevice <- function(name, type, res) {
    # Get path where plots will be generated.
    plotsPath <- .ps.graphics.plotSnapshotRoot("current-plot.png")
    ensure_parent_directory(plotsPath)

    if (is.null(type)) {
        type <- default_device_type()
    }

    # Create the graphics device.
    # TODO: Use 'ragg' if available?
    withCallingHandlers(
        grDevices::png(
            filename = plotsPath,
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
}

# Create a snapshot of the current plot.
#
# This saves the plot's display list, so it can be used
# to re-render plots as necessary.
#' @export
.ps.graphics.createSnapshot <- function(id) {
    # Flush any pending plot actions.
    grDevices::dev.set(grDevices::dev.cur())
    grDevices::dev.flush()

    # Create the plot snapshot.
    recordedPlot <- grDevices::recordPlot()

    # Get the path to the plot snapshot file.
    snapshotPath <- .ps.graphics.plotSnapshotPath(id)

    # Save it to disk.
    saveRDS(recordedPlot, file = snapshotPath)

    # Return the path to that snapshot file.
    snapshotPath
}

#' @export
.ps.graphics.renderPlot <- function(id, width, height, dpr, format) {
    # If we have an existing snapshot, render from that file.
    snapshotPath <- .ps.graphics.plotSnapshotPath(id)
    if (file.exists(snapshotPath)) {
        .ps.graphics.renderPlotFromSnapshot(id, width, height, dpr, format)
    } else {
        .ps.graphics.renderPlotFromCurrentDevice(id, width, height, dpr, format)
    }
}

#' @export
.ps.graphics.renderPlotFromSnapshot <- function(
    id,
    width,
    height,
    dpr,
    format
) {
    # Get path to snapshot file + output path.
    outputPath <- .ps.graphics.plotOutputPath(id)
    snapshotPath <- .ps.graphics.plotSnapshotPath(id)

    # Read the snapshot data.
    recordedPlot <- readRDS(snapshotPath)

    # Get device attributes to be passed along.
    type <- default_device_type()
    res <- .ps.graphics.defaultResolution * dpr
    width <- width * dpr
    height <- height * dpr

    # Create a new graphics device.
    renderWithPlotDevice(outputPath, format, width, height, res, type)

    # Replay the plot.
    suppressWarnings(grDevices::replayPlot(recordedPlot))

    # Turn off the device (commit the plot to disk)
    grDevices::dev.off()

    # Return path to generated plot file.
    invisible(outputPath)
}

#' @export
.ps.graphics.renderPlotFromCurrentDevice <- function(
    id,
    width,
    height,
    dpr,
    format
) {
    # Try and force the graphics device to sync changes.
    grDevices::dev.set(grDevices::dev.cur())
    grDevices::dev.flush()

    # Get the file name associated with the current graphics device.
    device <- .Devices[[grDevices::dev.cur()]]

    # Get device attributes to be passed along.
    type <- attr(device, "type") %??% default_device_type()
    res <- .ps.graphics.defaultResolution * dpr
    width <- width * dpr
    height <- height * dpr

    # Copy to a new graphics device.
    # TODO: We'll want some indirection around which graphics device is selected here.
    filepath <- attr(device, "filepath")
    grDevices::dev.copy(function() {
        renderWithPlotDevice(filepath, format, width, height, res, type)
    })

    # Turn off the graphics device.
    grDevices::dev.off()

    # Return path to the generated file.
    invisible(filepath)
}
