import_rstudioapi_shims <- function(path) {
    env <- rstudioapi_shims_env()
    source(path, local = env)
}

rstudioapi_shims_env <- function() {
    if (!"tools:rstudio" %in% search()) {
        # Create environment for the search path where we'll store our shims
        attach(list(), name = "tools:rstudio")

        # Override `rstudioapi::isAvailable()`
        setHook(
            packageEvent("rstudioapi", "onLoad"),
            function(...) {
                ns <- asNamespace("rstudioapi")

                unlockBinding("isAvailable", ns)
                on.exit(lockBinding("isAvailable", ns))

                # TODO: Should check for `version_needed`
                body(ns$isAvailable) <- TRUE
            }
        )
    }

    as.environment("tools:rstudio")
}
