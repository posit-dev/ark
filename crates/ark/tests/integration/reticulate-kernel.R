# Invoked by the reticulate integration test, once per environment and bundle mode.
args <- commandArgs(trailingOnly = TRUE)
stopifnot(length(args) == 3L, dir.exists(args[[1L]]), file.exists(args[[2L]]))
files <- normalizePath(args[[1L]], winslash = "/")
mode <- match.arg(args[[3L]], c("project", "managed", "unbundled"))
bundled <- mode != "unbundled"

library(reticulate)
host <- new.env()
sys.source("crates/ark/src/modules/positron/reticulate.R", host)
host$.ps.ark.version <- function() c(version = "test")
attach(as.list(host, all.names = TRUE), name = "tools:positron")
stopifnot(identical(py_require()$packages, c("numpy", "ipykernel")))

if (mode == "project") {
    venv <- tempfile("reticulate-project-")
    processx::run(args[[2L]], c("-m", "venv", "--without-pip", venv))
    executable <- if (.Platform$OS.type == "windows") {
        "Scripts/python.exe"
    } else {
        "bin/python"
    }
    Sys.setenv(RETICULATE_PYTHON = file.path(venv, executable))
    py_require(character(), action = "set")
} else {
    Sys.setenv(RETICULATE_PYTHON = "managed")
    py_require(python_version = "==3.14.*")
}
config <- .ps.rpc.reticulate_check_prerequisites()
initialized <- py_config()
main_before <- py_eval("list(globals())")
stopifnot(
    is.null(config$error),
    identical(.ps.rpc.reticulate_check_prerequisites(), config),
    identical(py_eval("list(globals())"), main_before),
    identical(
        config$embeddedInterpreter$version,
        list(major = 3L, minor = 14L)
    ),
    identical(config$embeddedInterpreter$implementation, "cpython"),
    identical(
        config$embeddedInterpreter$architecture,
        if (.Platform$OS.type == "windows") {
            sub("^win-", "", import("sysconfig")$get_platform())
        } else {
            tolower(import("platform")$machine())
        }
    ),
    !py_eval("'ipykernel' in __import__('sys').modules")
)

# Windows can run x64 Python on an ARM64 host. The RPC must report Python's architecture.
check_windows_architecture <- function() {
    patch <- import("unittest.mock")$patch
    patches <- list(
        patch("sys.platform", "win32"),
        patch("platform.machine", return_value = "ARM64"),
        patch("sysconfig.get_platform", return_value = "win-amd64")
    )
    on.exit(
        for (p in rev(patches)) {
            p$stop()
        }
    )
    for (p in patches) {
        p$start()
    }
    stopifnot(identical(
        .ps.rpc.reticulate_check_prerequisites()$embeddedInterpreter$architecture,
        "amd64"
    ))
}
check_windows_architecture()

# Inspect the environment in a separate process so bundled metadata cannot hide
# an installation into the project or a change to the managed environment.
inventory <- function() {
    processx::run(
        config$python,
        c(
            "-c",
            paste0(
                "import importlib.metadata, json; ",
                "print(json.dumps(sorted((d.metadata['Name'], d.version) ",
                "for d in importlib.metadata.distributions())))"
            )
        )
    )$stdout
}
before <- inventory()
if (mode == "project") {
    stopifnot(identical(trimws(before), "[]"))
}

paths <- character()
if (bundled) {
    arch <- switch(
        config$embeddedInterpreter$architecture,
        arm64 = ,
        aarch64 = "arm64",
        x86_64 = ,
        amd64 = "x64",
        stop("Unsupported architecture")
    )
    cpx_arch <- if (Sys.info()[["sysname"]] == "Darwin") "universal2" else arch
    paths <- file.path(
        files,
        "lib/ipykernel",
        c(
            paste0(cpx_arch, "/cp314"),
            paste0(arch, "/cp3"),
            "py3"
        )
    )
    stopifnot(all(dir.exists(paths)))

    # Two modules with the same name make import-path precedence observable.
    probes <- file.path(tempdir(), c("first", "second"))
    for (probe in probes) {
        dir.create(probe)
        writeLines(
            paste0("origin = '", basename(probe), "'"),
            file.path(probe, "bundle_probe.py")
        )
    }
    paths <- c(probes, paths)
}

py_run_string("existing_answer = 42")
pythonpath <- Sys.getenv("PYTHONPATH", unset = NA_character_)
connection <- tempfile(fileext = ".json")
result <- .ps.rpc.reticulate_start_kernel(
    file.path(files, "posit/positron_language_server.py"),
    connection,
    tempfile(fileext = ".log"),
    "debug",
    paths
)
stopifnot(identical(result, ""))
roots <- if (bundled) paths else import("sys")$prefix
roots <- paste0(normalizePath(roots, winslash = "/"), "/")
for (module in c("ipykernel", "zmq", "psutil", "tornado")) {
    origin <- normalizePath(import(module)$`__file__`, winslash = "/")
    stopifnot(any(startsWith(origin, roots)))
}
if (bundled) {
    stopifnot(identical(import("bundle_probe")$origin, "first"))
}

client <- processx::run(
    config$python,
    c("crates/ark/tests/integration/reticulate-client.py", connection),
    env = if (bundled) {
        c(PYTHONPATH = paste(paths, collapse = .Platform$path.sep))
    },
    timeout = 60000
)
cat(client$stdout)
stopifnot(
    identical(py_eval("bundle_answer"), 42L),
    identical(py_eval("existing_answer"), 42L),
    identical(inventory(), before),
    identical(py_config(), initialized),
    identical(Sys.getenv("PYTHONPATH", unset = NA_character_), pythonpath)
)
cat("Reticulate", mode, "kernel regression passed\n")
