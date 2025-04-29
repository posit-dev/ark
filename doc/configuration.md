# Introduction

This guide details Ark's configuration.

# R global variables

## ark.ragg

A boolean, with a default of `TRUE`.

If `TRUE` and if ragg \>=1.4.0 is installed, then ragg will be used for graphics generation. In particular:

-   ragg will be used for generating the display list of "plot instructions" regardless of the render type.

-   ragg will be used for generating graphics for png, jpeg, and tiff formats.

Otherwise, the corresponding device from grDevices will be used.

This global variable is only checked once per session, we recommend that you place it in an `.Rprofile` if you need to modify it.

## ark.resource_namespaces

A boolean, with a default value of `TRUE`.

If `TRUE`, virtual documents will be generated for packages without source references for usage during debugging.

## positron.error_entrace

A boolean, with a default value of `TRUE`.

If `TRUE`, errors will be entraced by `rlang::entrace()` if rlang is installed. This often results in more informative backtraces.
