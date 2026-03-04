#
# package.R
#
# Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
#
#

# Checks if a package is installed without loading it.
# Could be slow on network drives.
#' @export
.ps.is_installed <- function(pkg, minimum_version = NULL) {
    installed <- system.file(package = pkg) != ""

    if (installed && !is.null(minimum_version)) {
        installed <- utils::packageVersion(pkg) >= minimum_version
    }

    installed
}

#' @export
.ps.rpc.is_installed <- .ps.is_installed

# Returns a list containing:
#   * the version string if the package is installed and NULL otherwise
#   * a logical indicating if package is installed at or above the minimum version
#  This may seem weird, but it's impractical for positron-r to do version
#  comparisons.
#' @export
.ps.rpc.packageVersion <- function(pkg, minimumVersion = NULL) {
    installed <- system.file(package = pkg) != ""

    if (installed) {
        version <- utils::packageVersion(pkg)
        list(
            version = as.character(version),
            compatible = is.null(minimumVersion) || version >= minimumVersion
        )
    } else {
        list(
            version = NULL,
            compatible = FALSE
        )
    }
}

#' @export
.ps.rpc.install_packages <- function(packages) {
    packages <- unlist(packages)
    for (pkg in packages) {
        if (.ps.rpc.isPackageAttached(pkg)) {
            stop("Should not install a package if it's already attached.")
        }
    }
    utils::install.packages(packages)
    TRUE
}

#' @export
.ps.rpc.pkg_install <- function(packages, method = c("pak", "base")) {
    packages <- unlist(packages)
    method <- match.arg(method)
    switch(
        method,
        pak = pak::pkg_install(packages, ask = FALSE),
        base = utils::install.packages(packages)
    )
    TRUE
}

#' @export
.ps.rpc.pkg_list <- function(method = c("pak", "base")) {
    method <- match.arg(method)
    switch(
        method,
        pak = {
            old_opt <- options(pak.no_extra_messages = TRUE)
            on.exit(options(old_opt), add = TRUE)
            pkgs <- pak::lib_status()
            lapply(seq_len(nrow(pkgs)), function(i) {
                list(
                    id = paste0(pkgs$package[[i]], "-", pkgs$version[[i]]),
                    name = pkgs$package[[i]],
                    displayName = pkgs$package[[i]],
                    version = as.character(pkgs$version[[i]])
                )
            })
        },
        base = {
            ip <- utils::installed.packages()
            lapply(seq_len(nrow(ip)), function(i) {
                list(
                    id = paste0(ip[i, "Package"], "-", ip[i, "Version"]),
                    name = ip[i, "Package"],
                    displayName = ip[i, "Package"],
                    version = ip[i, "Version"]
                )
            })
        }
    )
}

#' @export
.ps.rpc.pkg_update_all <- function(method = c("pak", "base")) {
    method <- match.arg(method)
    switch(
        method,
        pak = {
            old_opt <- options(pak.no_extra_messages = TRUE)
            on.exit(options(old_opt), add = TRUE)
            outdated <- utils::old.packages()[, "Package"]
            if (length(outdated) > 0) {
                pak::pkg_install(outdated, ask = FALSE)
            }
        },
        base = utils::update.packages(ask = FALSE)
    )
    TRUE
}

#' @export
.ps.rpc.pkg_uninstall <- function(packages, method = c("pak", "base")) {
    packages <- unlist(packages)
    method <- match.arg(method)
    switch(
        method,
        pak = pak::pkg_remove(packages),
        base = utils::remove.packages(packages)
    )
    for (pkg in packages) {
        try(unloadNamespace(pkg), silent = TRUE)
    }
    TRUE
}

#' @export
.ps.rpc.pkg_search <- function(query, method = c("pak", "base")) {
    method <- match.arg(method)
    switch(
        method,
        pak = {
            old_opt <- options(pak.no_extra_messages = TRUE)
            on.exit(options(old_opt), add = TRUE)
            pkgs <- pak::pkg_search(query, size = 100)
            lapply(seq_len(nrow(pkgs)), function(i) {
                list(
                    id = pkgs$package[[i]],
                    name = pkgs$package[[i]],
                    displayName = pkgs$package[[i]],
                    version = "0"
                )
            })
        },
        base = {
            query <- tolower(query)
            ap <- utils::available.packages()
            matches <- ap[
                grepl(query, tolower(ap[, "Package"]), fixed = TRUE),
                ,
                drop = FALSE
            ]
            lapply(seq_len(nrow(matches)), function(i) {
                list(
                    id = matches[i, "Package"],
                    name = matches[i, "Package"],
                    displayName = matches[i, "Package"],
                    version = "0"
                )
            })
        }
    )
}

#' @export
.ps.rpc.pkg_search_versions <- function(name) {
    ap <- utils::available.packages()
    version <- if (name %in% rownames(ap)) ap[name, "Version"] else character(0)
    # Wrap in as.list() to ensure it serializes as an array, not a scalar
    as.list(version)
}

#' @export
.ps.rpc.isPackageAttached <- function(pkg) {
    if (!is_string(pkg)) {
        stop("`pkg` must be a string.")
    }

    pkg %in% .packages()
}

#' @export
.ps.rpc.get_attached_packages <- function(...) {
    .packages()
}

#' Get all installed RStudio addins
#' @export
.ps.rpc.getAddins <- function() {
    pkgs <- .packages(all.available = TRUE)
    out <- lapply(pkgs, get_package_addins)
    unlist(out, recursive = FALSE)
}

get_package_addins <- function(pkg) {
    path <- system.file("rstudio", "addins.dcf", package = pkg)
    if (!nzchar(path)) {
        return(list())
    }

    dcf <- tryCatch(read.dcf(path), error = function(cnd) {
        log_error(sprintf(
            "Failed to read addins.dcf for '%s': %s",
            pkg,
            conditionMessage(cnd)
        ))
        NULL
    })
    if (is.null(dcf)) {
        return(list())
    }

    cols <- colnames(dcf)
    out <- lapply(seq_len(nrow(dcf)), function(i) {
        parse_addin_row(dcf, i, cols, pkg)
    })
    out[lengths(out) > 0]
}

parse_addin_row <- function(dcf, row, cols, pkg) {
    binding <- dcf_field(dcf, row, "Binding", cols)
    if (!nzchar(binding)) {
        return(NULL)
    }

    list(
        name = dcf_field(dcf, row, "Name", cols),
        description = dcf_field(dcf, row, "Description", cols),
        binding = binding,
        interactive = identical(
            tolower(dcf_field(dcf, row, "Interactive", cols, "true")),
            "true"
        ),
        package = pkg
    )
}

#' Get all installed R Markdown templates
#' @export
.ps.rpc.getRmdTemplates <- function() {
    pkgs <- .packages(all.available = TRUE)
    out <- lapply(pkgs, get_package_templates)
    unlist(out, recursive = FALSE)
}

get_package_templates <- function(pkg) {
    path <- system.file("rmarkdown", "templates", package = pkg)
    if (!nzchar(path)) {
        return(list())
    }

    dirs <- list.dirs(path, recursive = FALSE, full.names = TRUE)
    out <- lapply(dirs, function(dir) parse_template_dir(dir, pkg))
    out[lengths(out) > 0]
}

parse_template_dir <- function(dir, pkg) {
    yaml_path <- file.path(dir, "template.yaml")
    if (!file.exists(yaml_path)) {
        return(NULL)
    }

    meta <- tryCatch(yaml::read_yaml(yaml_path), error = function(cnd) {
        log_error(sprintf(
            "Failed to read template.yaml at '%s': %s",
            yaml_path,
            conditionMessage(cnd)
        ))
        NULL
    })
    if (is.null(meta)) {
        return(NULL)
    }

    list(
        name = meta$name %||% basename(dir),
        description = meta$description %||% "",
        create_dir = isTRUE(meta$create_dir),
        package = pkg,
        template = basename(dir)
    )
}

# Safely get a field from a DCF matrix, returning default if missing or NA
dcf_field <- function(dcf, row, field, cols, default = "") {
    if (field %in% cols) {
        value <- dcf[row, field]
        if (is.na(value)) default else value
    } else {
        default
    }
}
