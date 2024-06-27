Some packages require fixes for better integration in Ark. We recommend running the following to make sure you're up-to-date:

```r
# Important
pak::pak("cli", "r-lib/roxygen2")

# Nice to have
pak::pak(c("rlang", "r-lib/pkgload"))
```

### roxygen2:

- Set `cli.hyperlink = FALSE` in `local_reproducible_output()`: <https://github.com/r-lib/roxygen2/pull/1621>
    - Bugfix for <https://github.com/posit-dev/positron/issues/3053>
    - Ark warns on load because this bug disruptive to dev workflow
    - **Unreleased**

- Use more generic x-r-run over `ide:run`: <https://github.com/r-lib/roxygen2/pull/1604>
    - Enhancement: Hyperlink support
    - **Unreleased**


### cli

- Change semantics of `cli.default_num_colors` option: <https://github.com/r-lib/cli/pull/625>
    - Bugfix for ANSI support in console: <https://github.com/posit-dev/positron/issues/1032>
    - Fixed in cli 3.6.2 (2023-12-11)


### rlang

- Use modern x-r-run convention: <https://github.com/r-lib/rlang/pull/1678>
    - Enhancement
    - Fixed in rlang 1.1.4 (2024-06-04)

- Display error prefixes in red experimentally: <https://github.com/r-lib/rlang/commit/29599cc5>
    - Enhancement
    - Fixed in rlang 1.1.3 (2024-01-10)

- Fix off by one typo in source column location: <https://github.com/r-lib/rlang/pull/1633>
    - Enhancement
    - Fixed in rlang 1.1.2 (2023-11-04)


### pkgload

- Check if rstudioapi is available <https://github.com/r-lib/pkgload/pull/277>
    - Enhancement: Support for dev help implemented in <https://github.com/posit-dev/ark/pull/347>
    - **Unreleased**

- Pass library path in user hook <https://github.com/r-lib/pkgload/commit/b4e178bd>
    - Bugfix for an error during `load_all()` due to an interaction with our custom onload hook
    - Worked around in <https://github.com/posit-dev/ark/commit/a03766e2>
    - **Unreleased**

- Fix NSE in `shim_help(package = )` <https://github.com/r-lib/pkgload/pull/267>
    - Bugfix for dev help
    - Worked around in <https://github.com/posit-dev/ark/pull/233>
    - **Unreleased**
