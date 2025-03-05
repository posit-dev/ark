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

Ark creates a "shadow" graphics device that manages other graphics devices. We technically wrap a `grDevices::png()` and inherit all of the default hooks and behaviors that come with that, and then we inject our own hooks on top of specific png hooks. For example, when the `newPage` hook is called we call the png device's `newPage` hook manually and then also layer in our own `newPage` behavior. This is how we learn when changes have occurred, when to record a plot, when we are deactivated, etc.

# Device interactivity

Certain plotting functions like `stats:::plot.lm()` will draw multiple plots in a row, pausing for the user to hit enter before advancing to the next page. This requires a few things to work properly:

-   `grDevices::deviceIsInteractive(name)` must be called during Ark graphics device initialization with `name` set to our Ark graphics device name to "register" ourselves as a known interactive graphics device.

-   `grDevices::dev.interactive()` should then return `TRUE`, and this is used in the default value of the `ask` argument of `stats:::plot.lm()`.

-   We must be able to render plots (i.e. replay plot recordings) at any time. In particular, we must be able to handle a render request from Positron at interrupt time. We currently wait until the next idle time, which happens after the user has hit enter and advanced through all the plots - at which point we actually render them all at once, which is a suboptimal experience. We've been actively avoiding doing much at interrupt time because it is rather unsafe to do so, and here we'd have to change graphics devices at interrupt time, which feels very unsafe.

    -   One way we could possibly improve on this is to render an initial version of each plot in the opening notification we send to Positron using some default dimensions and device type. Positron could also supply us these defaults to use during the startup procedure.

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

## Graphics system

There are two graphics systems in existence today: `base` and `grid`. Up to 16 are allowed in total.

Graphics systems build R interfaces on top of the lower level graphics devices, with the graphics engine sitting between the two and adding some common glue between them.

## New page

A new "page" roughly corresponds to what we actually end up showing the user. The easiest way to think about this is as a blank white piece of paper that you get to write graphics onto. Starting a new page via the `newPage` graphics device hook gives you a new blank piece of paper to write onto. At the R level, this is generally controlled by a call to `plot.new()` in the `base` graphics system, and `grid.newpage()` in the `grid` graphics system.

### before.plot.new / before.grid.newpage

Of note are the two `setHook()` hooks, [`before.plot.new`](https://stat.ethz.ch/R-manual/R-devel/library/graphics/html/frame.html) and [`before.grid.newpage`](https://stat.ethz.ch/R-manual/R-devel/library/grid/html/grid.newpage.html). Hooking into these gives us a chance to take an action *before* the plot frame advances. This is extremely useful to us, as it gives us a chance to "record" a plot's *instructions* (technically, its display list) so we can replay those instructions later on. We record the instructions with `recordPlot()` and replay them with `replayPlot()`. This is important for us because when we hit a new page event we tell Positron that we have a plot ready for it, but we don't actually send over the plot until Positron tells us what the dimensions of it should be, and what graphics device to use. While we are waiting for Positron to give us that information, other R code could be executing in the meantime. So we have to record the plot's instructions before the new page event so we can replay those instructions at the time when Positron responds with the rest of the information about exactly what plot to produce.
