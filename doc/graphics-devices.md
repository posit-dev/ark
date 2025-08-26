# Introduction

This guide describes how Ark's graphic device works, along with a primer on R's graphic devices in general.

A much more detailed description of everything graphics device related is in [R Internals](https://cran.r-project.org/doc/manuals/r-devel/R-ints.html#Graphics-Devices), so you if need to do deep work here, it is worth spending time reading about that.

# Lifecycle of a plot

Let's take a look at what generally happens when a user runs `plot(1:10)`:

-   If the graphics device has never been used, a new page is created. This runs our `hook_new_page()`, creating a new unique `id` for this plot page.

-   The graphics device "draws" the plot, which repeatedly triggers `hook_mode()` letting us know when drawing starts and stops, and as a side effect this lets us know that we have some changes to render. To Ark, "drawing" the plot looks like writing a series of *instructions* on how to create the plot to the graphics device's *display list*.

-   When either the top level code finishes executing (i.e. `plot(1:10)` is done), or when a new page is about to be created (i.e. triggering our `before.new.page` or `before.grid.newpage` hooks), we check to see if there are any changes to record for this plot `id` using `process_changes()`. If something has changed for this `id`:

    -   We *record* the instructions written in the display list using `grDevices::recordPlot()` and save the recording in a persistent `RECORDINGS` list, keyed by the `id`.

    -   We notify Positron that there is something to render (but note that we don't send the plot data over yet). This is either:

        -   A new plot notification, which we do by creating a new `CommSocket` and sending Positron a message containing the plot `id`. The `CommSocket` lets us have two way communication with Positron about that plot `id`.

        -   An update plot notification, which we do by sending a `PlotFrontendEvent::Update` message over the `CommSocket`.

-   At idle time, we eagerly run `process_rpc_requests()` to check if Positron has responded to our plot notifications, letting us know it is "ready" for us to send over the full plot data. At this time, Positron also provides the `width`, `height`, `pixel_ratio`, and `format` (i.e. graphics device) that we should use when rendering the plot. When we get one of these responses from Positron, we render the plot by:

    -   On the R side:

        -   Looking up the *recording* in `RECORDINGS` by its `id`

        -   Creating the graphics device requested by `format` (png, svg, etc) with the dimensions provided by Positron and with a `filepath` unique to this plot `id`.

        -   *Replaying* the recording with `grDevices::replayPlot()`, writing it into the newly opened graphics device.

        -   Closing that graphics device, which writes the plot to `filepath`.

    -   On the Rust side:

        -   Reading in the bytes from the `filepath`

        -   Encoding it at base64 and collecting the correct `mimetype`

        -   Sending the plot data and mimetype back to Positron along the `CommSocket` as a `PlotBackendReply::RenderReply`

-   Positron shows this plot data to the user, either in the viewer, in an editor, or by saving it to disk for the user.

-   If the user resizes the plots pane or requests an export to file, we get a new RPC request for that plot `id`, and we go through the process of re-rendering it again with the recorded display list. This feature is known internally as having "dynamic" plots, see `should_use_dynamic_plots()`.

For pure Jupyter settings outside of Positron, we don't have "dynamic" plots. Instead when we see that we have changes in `process_changes()`, we record the display list and then immediately render it to the specified device and send the result as either a `IOPubMessage::DisplayData` or `IOPubMessage::UpdateDisplayData` message, depending on whether or not it was a new plot.

# Ark graphics device

Ark creates a "shadow" graphics device that manages other graphics devices. We technically wrap `grDevices::png()` or `ragg::agg_record()` and inherit all of the default hooks and behaviors that come with that, and then we inject our own hooks on top of specific png hooks. For example, when the `newPage` hook is called we call the device's `newPage` hook manually and then also layer in our own `newPage` behavior. This is how we learn when changes have occurred, when to record a plot, when we are deactivated, etc.

We prefer using the ragg device if it is available, as it is cheaper and has better behavior with some features that surprisingly do still use the underlying device, like rendering of Chinese text in a plot.

# Device interactivity

Certain plotting functions like `stats:::plot.lm()` or `demo(graphics)` will draw multiple plots in a row, pausing for the user to hit enter before advancing to the next page. This requires a few things to work properly:

-   Internally, `ARK_GRAPHICS_DEVICE_NAME` is set to `".ark.graphics.device"`, which matches the name of a function we expose named `.ark.graphics.device()`, which is in charge of creating a new Ark graphics device and is findable by `get0(".ark.graphics.device", globalenv(), inherits = TRUE)`.

-   `grDevices::deviceIsInteractive(ARK_GRAPHICS_DEVICE_NAME)` must be called during startup to "register" ourselves as a known interactive graphics device.

-   `options(device = ARK_GRAPHICS_DEVICE_NAME)` must be called during startup so that:

    -   `dev.new()` and `GECurrentDevice()` know the function name to look up to create a new device when the current is `"null device"`.

    -   `grDevices::dev.interactive(orNone = TRUE)` has a name to look up in the list returned from `deviceIsInteractive()` in a fresh session where the device is otherwise still `"null device"`. This is used by `demo(graphics)`.

-   When we send Positron a notification about a new plot, we *also* send along an initial version of that plot to show the user. Even if our dimensions are wrong, this allows Positron to show the user a plot after each `Enter` keypress, where ark is otherwise paused waiting for the user to do something and cannot handle a render request.

With all of that in place:

-   `grDevices::dev.interactive(orNone = TRUE)` should always return `TRUE`, even in a fresh session when the current device is technically `"null device"`.

-   `grDevices::dev.interactive()` should return `TRUE` after you've created your first plot (before then, the device is `"null device"` and this returns `FALSE`, this matches RStudio).

    -   This is used in the default value of the `ask` argument of `stats:::plot.lm()` and by the time `ask` is evaluated our device has been created so this returns `TRUE` there as intended.

Note that with this approach, a motivated user can still set `options(device = "quartz")` in their `.Rprofile` if they'd like to use their own default graphics device rather than the one that comes with ark / Positron.

# Structures and terminology

## Graphics device

The absolute lowest level of R's graphics system.

This is where most of the work lives that is done by devices like png, jpeg, ragg, etc.

A single graphics device contributes a set of "graphics primitives" for doing things like drawing a circle, or a line, or text. In addition, relevant to Ark is the fact that it also implements hooks for things like:

-   Starting a new "page" to write to (`newPage` hook)

-   Knowing if a device is "current" or stops being "current" (`activate` and `deactivate` hooks)

-   Knowing when drawing starts or finishes (`mode` hook, `1` is drawing, `0` is not drawing)

A graphics device is represented by the C struct `DevDesc`, and is seen in our code as `pDevDesc`, a pointer to that struct. The definitions of this struct lives in [R_ext/GraphicsDevice.h](https://github.com/wch/r-source/blob/trunk/src/include/R_ext/GraphicsDevice.h).

## Graphics engine

A graphics engine maintains an array of graphics devices. The graphics devices themselves are wrapped by the C struct `GEDevDesc`, which is laid out like:

```
struct GEDevDesc {
  pDevDesc dev;
  ...
}
```

You'll see `pGEDevDesc` in the code representing a pointer to one of these device wrappers owned by the graphics engine. The definitions of this struct lives in [R_ext/GraphicsEngine.h](https://github.com/wch/r-source/blob/trunk/src/include/R_ext/GraphicsEngine.h).

An easy way to think about ownership is that anytime you see `GE` as a prefix on a function or struct, you as the graphics device do not manage that data, R does. For example, the `displayList` itself lives in `pGEDevDesc` and is managed by R, the device does not manage recording instructions here.

## Graphics system

There are two graphics systems in existence today: `base` and `grid`. Up to 16 are allowed in total.

Graphics systems build R interfaces on top of the lower level graphics devices, with the graphics engine sitting between the two and adding some common glue between them.

## New page

A new "page" roughly corresponds to what we actually end up showing the user. The easiest way to think about this is as a blank white piece of paper that you get to write graphics onto. Starting a new page via the `newPage` graphics device hook gives you a new blank piece of paper to write onto. At the R level, this is generally controlled by a call to `plot.new()` in the `base` graphics system, and `grid.newpage()` in the `grid` graphics system.

### before.plot.new / before.grid.newpage

Of note are the two `setHook()` hooks, [`before.plot.new`](https://stat.ethz.ch/R-manual/R-devel/library/graphics/html/frame.html) and [`before.grid.newpage`](https://stat.ethz.ch/R-manual/R-devel/library/grid/html/grid.newpage.html). Hooking into these gives us a chance to take an action *before* the plot frame advances. This is extremely useful to us, as it gives us a chance to "record" a plot's *instructions* (technically, its display list) so we can replay those instructions later on. We record the instructions with `recordPlot()` and replay them with `replayPlot()`. This is important for us because when we hit a new page event we tell Positron that we have a plot ready for it, but we don't actually send over the plot until Positron tells us what the dimensions of it should be, and what graphics device to use. While we are waiting for Positron to give us that information, other R code could be executing in the meantime. So we have to record the plot's instructions before the new page event so we can replay those instructions at the time when Positron responds with the rest of the information about exactly what plot to produce.
