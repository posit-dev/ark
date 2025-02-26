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

replayPlotWithDevice <- function(
    plot,
    filepath,
    format,
    width,
    height,
    res,
    type
) {
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

    # Replay the plot under this device.
    suppressWarnings(grDevices::replayPlot(plot))

    # Turn off the device (commits the plot to disk, and moves us back to
    # being the current device).
    grDevices::dev.off()
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
        type <- defaultDeviceType()
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
    # Get path to snapshot file + output path.
    outputPath <- .ps.graphics.plotOutputPath(id)
    snapshotPath <- .ps.graphics.plotSnapshotPath(id)

    if (!file.exists(snapshotPath)) {
        stop(sprintf(
            "Failed to render plot for plot `id` %s. Snapshot is missing.",
            id
        ))
    }

    # Read the snapshot data.
    recordedPlot <- readRDS(snapshotPath)

    # Get device attributes to be passed along.
    type <- defaultDeviceType()
    res <- .ps.graphics.defaultResolution * dpr
    width <- width * dpr
    height <- height * dpr

    # Replay the plot with the specified device.
    replayPlotWithDevice(
        recordedPlot,
        outputPath,
        format,
        width,
        height,
        res,
        type
    )

    # Return path to generated plot file.
    invisible(outputPath)
}
