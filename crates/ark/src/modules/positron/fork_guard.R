#
# fork_guard.R
#
# Copyright (C) 2026 Posit Software, PBC. All rights reserved.
#
#

# ark's IOPub/ZMQ background threads don't survive `fork()`
# (posit-dev/positron#3817). Every forking path in {parallel} funnels through
# the internal `mcfork()`, the only R-level caller of the C fork primitive. So
# shimming `mcfork()` with a stub that errors immediately blocks them all,
# pointing the user at backends that launch fresh subprocesses instead.
#
# We also shim `makeForkCluster()` directly. It wraps `mcfork()` in a
# `tryCatch()` and rethrows a generic "Cluster setup failed." on any error, so
# our message would never reach the user otherwise. `makeCluster(type = "FORK")`
# dispatches to `makeForkCluster()` through the namespace, so it is covered too.
#
# `mclapply()` and friends fall back to a serial `lapply()`/`mapply()` when
# `mc.cores = 1` or the input has fewer than two elements. Those paths never
# reach `mcfork()`, so they keep working and run serially, matching Windows
# where {parallel} ships serial stubs (`R/windows/mcdummies.R`) and never forks.

initialize_fork_guard <- function() {
    # These functions only fork on Unix. On Windows {parallel} never forks, so
    # there is nothing to guard.
    if (is_windows()) {
        return(invisible())
    }

    if (isNamespaceLoaded("parallel")) {
        bind_fork_guard_ns()
        if ("package:parallel" %in% search()) {
            bind_fork_guard_pkg()
        }
    }

    setHook(packageEvent("parallel", "onLoad"), function(...) {
        bind_fork_guard_ns()
    })
    setHook(packageEvent("parallel", "attach"), function(...) {
        bind_fork_guard_pkg()
    })
}

# `mcfork()` is internal, so the namespace binding covers every caller. Only
# `makeForkCluster()` is exported, so it also needs a package binding for
# unqualified calls after `library(parallel)`.
fork_guard_ns_names <- c("mcfork", "makeForkCluster")
fork_guard_pkg_names <- "makeForkCluster"

bind_fork_guard_ns <- function() {
    for (name in fork_guard_ns_names) {
        ns_bind("parallel", name, make_fork_guard_shim(name))
    }
}

bind_fork_guard_pkg <- function() {
    for (name in fork_guard_pkg_names) {
        pkg_bind("parallel", name, make_fork_guard_shim(name))
    }
}

make_fork_guard_shim <- function(name) {
    original <- utils::getFromNamespace(name, "parallel")

    # `ns_bind()`/`pkg_bind()` require the replacement to share the original's
    # formals.
    shim <- function() {}
    formals(shim) <- formals(original)
    body(shim) <- quote(stop_no_fork())

    shim
}

stop_no_fork <- function() {
    msg <- paste_line(c(
        sprintf("Can't fork the R session in %s.", app_name()),
        "Use a backend that starts fresh R processes instead: PSOCK clusters,",
        "`future::multisession()`, mirai, or `purrr::in_parallel()`."
    ))
    stop(msg, call. = FALSE)
}
