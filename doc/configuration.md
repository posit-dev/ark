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

Ark overrides a number of R options during startup, *after* sourcing the user's `.Rprofile`. When an option is `NULL`, we know the user hasn't set it and we apply our default. However, a subset of base R options have non-`NULL` defaults, which makes it impossible to reliably detect user overrides. For those options, you can list them in `ark.protected_options` to tell Positron to preserve the existing value (either R's default or the value set in Rprofile).

## `ark.protected_options`

A character vector of option names that Positron should leave alone. Set this in your `.Rprofile` to prevent Positron from overriding specific options. Accepted values are the option names listed under *Overrides* below. It can also be used for any of the options listed under *Defaults* to preserve their `NULL` default. For example:

```r
options(
  ark.protected_options = c("browser", "editor", "max.print")
)
```

This is an advanced option. Some Positron functionality might no longer work as expected when protected options are changed.

## Overrides

These base R options have non-`NULL` defaults, so we can't detect user overrides by checking for `NULL`. They are always set unless listed in `ark.protected_options`.

- **`editor`** — Opens files in the Positron editor. Used by `edit()`, `file.edit()`, etc.
- **`browser`** — Routes URLs through Positron's URL handler (`ps_browse_url`). Handles help pages, web URLs, and file paths.
- **`askpass`** — Prompts for passwords through Positron's UI. Same approach as RStudio (see `?rstudioapi::askForPassword`).
- **`connectionObserver`** — Integrates with Positron's Connections pane.
- **`device`** — Set to `".ark.graphics.device"`, the function name that `dev.new()` and `GECurrentDevice()` look for when creating a new graphics device.
- **`max.print`** — Set to `1000` (R's own default is `99999`). Limits console output to avoid overwhelming the display.

## Defaults

These options are only set when the user has *not* already defined them (e.g. in `.Rprofile`). This allows users to simply set the option to any value to override Positron's default. They can also be listed in `ark.protected_options` to keep the option as `NULL`.

- **`help_type`** — Set to `"html"`. Displays help pages as HTML rather than plain text.
- **`viewer`** — Displays HTML content in Positron's Viewer pane. Used by htmlwidgets, Shiny gadgets, etc.
- **`shiny.launch.browser`** — Set to a function that shows the Shiny app URL in Positron's Viewer pane. Set `options(shiny.launch.browser = TRUE)` in your `.Rprofile` to open Shiny apps in the system browser instead. Note that `TRUE` causes Shiny to use the `browser` option, which defaults to Positron's viewer, so you may also want to protect `browser`.
- **`plumber.docs.callback`** — Set to a function that shows Plumber API docs in Positron's Viewer pane.
- **`profvis.print`** — Set to a function that renders profvis output and displays it in Positron's Viewer pane.
