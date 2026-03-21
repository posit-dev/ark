# Introduction

This guide details Ark's configuration.

# Ark options

## `ark.ragg`

A boolean, with a default of `TRUE`.

If `TRUE` and if ragg \>=1.4.0 is installed, then ragg will be used for graphics generation. In particular:

-   ragg will be used for generating the display list of "plot instructions" regardless of the render type.

-   ragg will be used for generating graphics for png, jpeg, and tiff formats.

Otherwise, the corresponding device from grDevices will be used.

This option is only checked once per session, we recommend that you place it in an `.Rprofile` if you need to modify it.

## `ark.svglite`

A boolean, with a default value of `TRUE`.

If `TRUE` and if svglite is installed, then svglite will be used for rendering SVG graphics.

Otherwise, `grDevices::svg()` will be used.

This option is only checked once per session, we recommend that you place it in an `.Rprofile` if you need to modify it.

## `ark.resource_namespaces`

A boolean, with a default value of `TRUE`.

If `TRUE`, virtual documents will be generated for packages without source references for usage during debugging.

# Positron options

## `positron.show_last_value`

A boolean, with a default value of `FALSE`.

If `TRUE`, the special `.Last.value` will be shown in Positron's Variables pane.

## `positron.error_entrace`

A boolean, with a default value of `TRUE`.

If `TRUE`, errors will be entraced by `rlang::entrace()` if rlang is installed. This often results in more informative backtraces.

# R options

Ark overrides a number of R options during startup, *after* sourcing the user's `.Rprofile`. Depending on whether we can detect a user override, options are either set unconditionally (with an `I()` escape hatch) or only when the current value is `NULL`.

## Set unless escaped with `I()`

These options have non-`NULL` defaults in R, so we can't detect user overrides by checking for `NULL`. They are always set unless the user wraps their value in `I()` in their `.Rprofile`, e.g. `options(browser = I(Sys.getenv("R_BROWSER")))`.

- **`editor`** — Opens files in the Positron editor. Used by `edit()`, `file.edit()`, etc.
- **`browser`** — Routes URLs through Positron's URL handler (`ps_browse_url`). Handles help pages, web URLs, and file paths. Set `options(browser = I(Sys.getenv("R_BROWSER")))` to use the system browser instead.
- **`askpass`** — Prompts for passwords through Positron's UI. Same approach as RStudio (see `?rstudioapi::askForPassword`).
- **`connectionObserver`** — Integrates with Positron's Connections pane.
- **`device`** — Set to `".ark.graphics.device"`, the function name that `dev.new()` and `GECurrentDevice()` look for when creating a new graphics device.
- **`max.print`** — Set to `1000` (R's own default is `99999`). Limits console output to avoid overwhelming the display.

## Set if not `NULL`

These options are only set when the user has *not* already defined them (e.g. in `.Rprofile`). This allows users to override Positron's defaults.

- **`help_type`** — Set to `"html"`. Displays help pages as HTML rather than plain text.
- **`viewer`** — Displays HTML content in Positron's Viewer pane. Used by htmlwidgets, Shiny gadgets, etc.

# Package options

Ark also overrides options from third-party packages. Same rules as above: applied *after* `.Rprofile`, user-defined values take precedence (these options are set if not `NULL`).

- **`shiny.launch.browser`** — Set to a function that shows the Shiny app URL in Positron's Viewer pane. Set `options(shiny.launch.browser = TRUE)` in your `.Rprofile` to open Shiny apps in the system browser instead. You will also need `options(browser = I(Sys.getenv("R_BROWSER")))` since `TRUE` causes Shiny to use the `browser` option, which defaults to Positron's viewer.
- **`plumber.docs.callback`** — Set to a function that shows Plumber API docs in Positron's Viewer pane.
- **`profvis.print`** — Set to a function that renders profvis output and displays it in Positron's Viewer pane.
