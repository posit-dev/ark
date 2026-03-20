#
# repos.R
#
# Copyright (C) 2024 Posit Software, PBC. All rights reserved.
#
#

# Private environment for PPM credential state (survives across function calls)
.ppm_state <- new.env(parent = emptyenv())

# Applies defaults to the repos option. Does not override any existing
# repositories set in the option.
#
# Arguments:
#   defaults: A named character vector of default CRAN repositories.
apply_repo_defaults <- function(
    defaults = c(CRAN = "https://cran.rstudio.com/")
) {
    repos <- getOption("repos")
    if (is.null(repos) || !is.character(repos)) {
        # There are no repositories set, so apply the defaults directly
        repos <- defaults
    } else {
        if ("CRAN" %in% names(repos) && "CRAN" %in% names(defaults)) {
            # If a CRAN repository is set to @CRAN@ or is marked as having been
            # updated by "the IDE" *and* a default provides a specific URL,
            # override it.
            #
            # This is the only instance in which we replace an already-set
            # repository option with a default.
            if (
                identical(repos[["CRAN"]], "@CRAN@") ||
                    isTRUE(attr(repos, "IDE"))
            ) {
                repos[["CRAN"]] <- defaults[["CRAN"]]

                # Flag this CRAN repository as being set by the IDE. This flag is
                # used by renv.
                attr(repos, "IDE") <- TRUE
            }
        }
        # Set all the names in the defaults that are not already set
        for (name in names(defaults)) {
            if (!(name %in% names(repos))) {
                repos[[name]] <- defaults[[name]]
            }
        }
    }
    options(repos = repos)
}

#' Set the Posit Package Manager repository
#'
#' Sets the Posit Package Manager repository URL for the current session. The
#' URL will be processed to point to a Linux distribution-specific binary URL if
#' applicable.
#'
#' This function only overrides the CRAN repository when Ark has previously set
#' it or when it uses placeholder `"@CRAN@"`.
#'
#' @param url A PPM repository URL. Must be in the form
#'   `"https://host/repo/snapshot"`, e.g.,
#'   `"https://packagemanager.posit.co/cran/latest"`.
#'
#' @return `NULL`, invisibly.
#'
#' @export
.ps.set_ppm_repo <- function(url) {
    # Strip trailing slash — Ark's URL parser requires exactly 2 path segments.
    url <- sub("/+$", "", url)
    # Use Ark's built-in PPM binary URL detection.
    url <- .ps.Call("ps_get_ppm_binary_url", url)
    apply_repo_defaults(c(CRAN = url))
}

#' Set PPM credentials for authenticated package downloads
#'
#' Configures R for authenticated downloads from Posit Package Manager.
#' Uses three mechanisms to cover all R download paths:
#'
#' 1. **Repo URL with embedded auth** — `http://__token__:JWT@host/path`.
#'    This is what pak, renv, and other tools that spawn subprocesses read
#'    from `getOption("repos")`. Works universally since URL-based Basic
#'    auth is handled by all HTTP clients.
#'
#' 2. **Patched `available.packages()`** — R caches package indices to
#'    temp files named after the repo URL. With a JWT in the URL the
#'    filename exceeds the OS 255-char limit. We patch `available.packages`
#'    to strip credentials before generating cache filenames.
#'
#' 3. **Patched `download.file()`** — On macOS, `install.packages()` forces
#'    the `"libcurl"` method for binary downloads, which doesn't read netrc
#'    or `download.file.extra`. We inject an Authorization header for URLs
#'    matching the PPM host.
#'
#' All state is in-memory only (R options + process environment). Nothing
#' is written to disk except temp files in `tempdir()` that R cleans up
#' on session exit.
#'
#' @param url A PPM repository URL (same format as `.ps.set_ppm_repo()`).
#' @param token A PPM access token (JWT).
#'
#' @return `NULL`, invisibly.
#'
#' @export
.ps.set_ppm_credentials <- function(url, token) {
    url <- sub("/+$", "", url)
    host <- sub("^https?://([^:/]+).*$", "\\1", url)
    auth_b64 <- .ps.Call("ps_base64_encode", paste0("__token__:", token))
    auth_value <- paste0("Basic ", auth_b64)

    # --- 1. Set repo URL with embedded auth (for pak/renv subprocesses) ---
    binary_url <- .ps.Call("ps_get_ppm_binary_url", url)
    authed_url <- sub("^(https?://)", paste0("\\1__token__:", token, "@"), binary_url)
    apply_repo_defaults(c(CRAN = authed_url))

    # --- Save state for patches ---
    .ppm_state$auth_host <- host
    .ppm_state$auth_value <- auth_value

    # --- 2. Patch available.packages() to strip auth from cache filenames ---
    if (is.null(.ppm_state$original_available.packages)) {
        .ppm_state$original_available.packages <- utils::available.packages
    }

    patched_available.packages <- function(contriburl = utils::contrib.url(getOption("repos"), type),
                                           method, fields = NULL,
                                           type = getOption("pkgType"),
                                           filters = NULL, repos = getOption("repos"), ...) {
        # Strip __token__:...@ from repo URLs so cache filenames stay short
        contriburl <- sub("__token__:[^@]+@", "", contriburl)
        if (missing(method)) {
            .ppm_state$original_available.packages(
                contriburl, fields = fields, type = type,
                filters = filters, repos = repos, ...)
        } else {
            .ppm_state$original_available.packages(
                contriburl, method, fields = fields, type = type,
                filters = filters, repos = repos, ...)
        }
    }
    .ppm_override_ns_fn("available.packages", patched_available.packages, "utils")

    # --- 3. Patch download.file() to inject auth header for libcurl ---
    if (is.null(.ppm_state$original_download.file)) {
        .ppm_state$original_download.file <- utils::download.file
    }

    patched_download.file <- function(url, destfile, method, quiet = FALSE,
                                      mode = "w", cacheOK = TRUE,
                                      extra = getOption("download.file.extra"),
                                      headers = NULL, ...) {
        if (any(grepl(.ppm_state$auth_host, url, fixed = TRUE))) {
            # Strip embedded creds from URL (we pass auth via header instead)
            url <- sub("__token__:[^@]+@", "", url)
            headers <- c(headers, Authorization = .ppm_state$auth_value)
        }
        if (missing(method)) {
            .ppm_state$original_download.file(
                url, destfile, quiet = quiet, mode = mode,
                cacheOK = cacheOK, extra = extra, headers = headers, ...)
        } else {
            .ppm_state$original_download.file(
                url, destfile, method, quiet = quiet, mode = mode,
                cacheOK = cacheOK, extra = extra, headers = headers, ...)
        }
    }
    .ppm_override_ns_fn("download.file", patched_download.file, "utils")

    # --- 4. Patch curl::new_handle to set httpauth + netrc (main session) ---
    # libcurl does NOT send Basic auth preemptively — httpauth = 1L is required.
    netrc_path <- file.path(tempdir(), ".ppm-netrc")
    netrc_entry <- paste("machine", host, "login __token__ password", token)
    writeLines(netrc_entry, netrc_path)
    Sys.chmod(netrc_path, "0600")
    .ppm_state$netrc_path <- netrc_path

    .ppm_patch_curl_new_handle(netrc_path)

    # --- 5. Inject curl patch into pak's subprocess (if already running) ---
    # pak reuses a persistent callr::r_session. Env vars set after creation
    # don't propagate. We inject the patch directly via pak:::remote().
    .ppm_patch_pak_subprocess(netrc_path)

    # Hook for when pak loads later in this session
    setHook(packageEvent("pak", "onLoad"), function(...) {
        # Delay slightly so pak's subprocess has time to initialize
        later <- tryCatch(getFromNamespace("later", "later"), error = function(e) NULL)
        if (!is.null(later)) {
            later(function() .ppm_patch_pak_subprocess(netrc_path), delay = 2)
        }
    })
}

# Patch curl::new_handle to set httpauth + netrc for PPM auth.
# libcurl requires httpauth = 1L (CURLAUTH_BASIC) to send credentials preemptively.
.ppm_patch_curl_new_handle <- function(netrc_path) {
    if (!requireNamespace("curl", quietly = TRUE)) return()

    if (is.null(.ppm_state$original_curl_new_handle)) {
        .ppm_state$original_curl_new_handle <- curl::new_handle
    }

    patched_new_handle <- function(...) {
        h <- .ppm_state$original_curl_new_handle(...)
        curl::handle_setopt(h, httpauth = 1L, netrc = 1L, netrc_file = netrc_path)
        h
    }
    .ppm_override_ns_fn("new_handle", patched_new_handle, "curl")
}

# Inject the curl auth patch into pak's running subprocess.
# pak uses a persistent callr::r_session that's already running,
# so we inject the patch via pak:::remote().
.ppm_patch_pak_subprocess <- function(netrc_path) {
    if (!isNamespaceLoaded("pak")) return()

    tryCatch({
        pak:::remote(function(npath) {
            if (!file.exists(npath)) return()
            orig <- curl::new_handle
            patched <- function(...) {
                h <- orig(...)
                curl::handle_setopt(h,
                    httpauth = 1L,
                    netrc = 1L,
                    netrc_file = npath)
                h
            }
            ns <- asNamespace("curl")
            unlockBinding("new_handle", ns)
            assign("new_handle", patched, envir = ns)
            lockBinding("new_handle", ns)
        }, list(npath = netrc_path))
    }, error = function(e) {
        # pak subprocess not ready yet or pak:::remote failed
    })
}

# Unlock, replace, and re-lock a function in a namespace.
.ppm_override_ns_fn <- function(name, fn, ns_name) {
    ns <- asNamespace(ns_name)
    unlockBinding(name, ns)
    assign(name, fn, envir = ns)
    lockBinding(name, ns)
}

#' Clear PPM credentials
#'
#' Strips auth from the repo URL, restores patched functions, and resets
#' download options.
#'
#' @return `NULL`, invisibly.
#'
#' @export
.ps.clear_ppm_credentials <- function() {
    # Strip credentials from repo URL
    repos <- getOption("repos")
    if (!is.null(repos) && "CRAN" %in% names(repos)) {
        repos[["CRAN"]] <- sub("__token__:[^@]+@", "", repos[["CRAN"]])
        options(repos = repos)
    }

    # Restore original available.packages
    if (!is.null(.ppm_state$original_available.packages)) {
        .ppm_override_ns_fn("available.packages", .ppm_state$original_available.packages, "utils")
        .ppm_state$original_available.packages <- NULL
    }

    # Restore original download.file
    if (!is.null(.ppm_state$original_download.file)) {
        .ppm_override_ns_fn("download.file", .ppm_state$original_download.file, "utils")
        .ppm_state$original_download.file <- NULL
    }

    # Restore original curl::new_handle
    if (!is.null(.ppm_state$original_curl_new_handle)) {
        .ppm_override_ns_fn("new_handle", .ppm_state$original_curl_new_handle, "curl")
        .ppm_state$original_curl_new_handle <- NULL
    }

    # Clean up temp netrc
    netrc_path <- file.path(tempdir(), ".ppm-netrc")
    if (file.exists(netrc_path)) unlink(netrc_path)

    .ppm_state$auth_host <- NULL
    .ppm_state$auth_value <- NULL
    .ppm_state$netrc_path <- NULL
}
