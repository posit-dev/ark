# Introduction

This guide details Ark's configuration.

# R global variables

## ark.ragg

A boolean, with a default of `TRUE`.

Should the ragg package be used for graphics generation?

If `TRUE` and if ragg \>=1.4.0 is installed, then ragg will be used. In particular:

-   ragg will be used for generating the display list of "plot instructions" regardless of the render type.

-   ragg will be used for generating graphics for png, jpeg, and tiff formats.

Otherwise, the corresponding device from grDevices will be used.

This global variable is only checked once per session, we recommend that you place it in an `.Rprofile` if you need to modify it.
