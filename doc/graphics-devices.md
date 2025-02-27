# Introduction

This guide describes how graphics devices generally work in R, at least with respect to how they interact with Ark.

A much more detailed description of everything graphics device related is in [R Internals](https://cran.r-project.org/doc/manuals/r-devel/R-ints.html#Graphics-Devices), so you if need to do deep work here, it is worth spending time reading about that.

# Structures and terminology

## Graphics device

The absolute lowest level of R's graphics system.

This is where most of the work lives that is done by devices like png, jpeg, ragg, etc.

A single graphics device contributes a set of "graphics primitives" for doing things like drawing a circle, or a line, or text. In addition, relevant to Ark is the fact that it also implements hooks for things like:

-   Starting a new "page" to write to (`newPage` hook)

-   Knowing if a device is "current" or stops being "current" (`activate` and `deactivate` hooks)

-   Knowing when drawing starts or finishes (`mode` hook, `1` is drawing, `0` is not drawing)

-   Knowing when a device closes

A graphics device is represented by the C struct `DevDesc`, and is seen in our code as `pDevDesc`, a pointer to that struct. The definitions of this struct lives in [R_ext/GraphicsDevice.h](https://github.com/wch/r-source/blob/trunk/src/include/R_ext/GraphicsDevice.h).

## Graphics engine

There are two graphics engines in existence today: `base` and `grid`.

Graphics engines build R interfaces on top of the lower level graphics devices.

A graphics engine maintains an array of graphics devices. The graphics devices themselves are wrapped by the C struct `GEDevDesc`, which is laid out like:

```
struct GEDevDesc {
  pDevDesc dev;
  ...
}
```

You'll see `pGEDevDesc` in the code representing a pointer to one of these device wrappers owned by the graphics engine. The definitions of this struct lives in [R_ext/GraphicsEngine.h](https://github.com/wch/r-source/blob/trunk/src/include/R_ext/GraphicsEngine.h).

## New page

A new "page" roughly corresponds to what we actually end up showing the user. The easiest way to think about this is as a blank white piece of paper that you get to write graphics onto. Starting a new page via the `newPage` graphics device hook gives you a new blank piece of paper to write onto. At the R level, this is generally controlled by a call to `plot.new()` in the `base` graphics engine, and `grid.newpage()` in the `grid` graphics engine.

### before.plot.new / before.grid.newpage

Of note are the two `setHook()` hooks, [`before.plot.new`](https://stat.ethz.ch/R-manual/R-devel/library/graphics/html/frame.html) and [`before.grid.newpage`](https://stat.ethz.ch/R-manual/R-devel/library/grid/html/grid.newpage.html). Hooking into these gives us a chance to take an action *before* the plot frame advances. This is extremely useful to us, as it gives us a chance to "snapshot" a plot's *instructions* (technically, its display list) so we can replay those instructions later on. We record the instructions with `recordPlot()` and replay them with `replayPlot()`. This is important for us because when we hit a new page event we tell Positron that we have a plot ready for it, but we don't actually send over the plot until Positron tells us what the dimensions of it should be, and what graphics device to use. While we are waiting for Positron to give us that information, other R code could be executing in the meantime. So we have to snapshot the plot's instructions before the new page event so we can replay those instructions at the time when Positron responds with the rest of the information about exactly what plot to produce.

## Device interactivity

Certain plotting functions like `stats:::plot.lm()` will draw multiple plots in a row, pausing for the user to hit enter before advancing to the next page. This requires a few things to work properly:

-   `grDevices::deviceIsInteractive(name)` must be called with `name` set to our Ark graphics device name to "register" ourselves as a known interactive graphics device.

-   `grDevices::dev.interactive()` should then return `TRUE`, and this is used in the default value of the `ask` argument of `stats:::plot.lm()`.

-   We must be able to render plots (replaying snapshots) at any time. In particular, we must be able to handle a render request from Positron when the user is sitting at the stdin prompt and being prompted to hit enter to see the next plot. We currently wait until the next idle time, which happens after the user has hit enter and advanced through all the plots - at which point we actually render them all at once, which is a suboptimal experience.

# Ark graphics device

Ark creates a "shadow" graphics device that manages other graphics devices. We technically wrap a `grDevices::png()` and inherit all of the default hooks and behaviors that come with that, and then we inject our own hooks on top of specific png hooks. For example, when the `newPage` hook is called we call the png device's `newPage` hook manually and then also layer in our own `newPage` behavior.
