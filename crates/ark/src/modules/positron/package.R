#
# package.R
#
# Copyright (C) 2023-2025 Posit Software, PBC. All rights reserved.
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
    for (pkg in packages) {
        if (.ps.rpc.isPackageAttached(pkg)) {
            stop("Should not install a package if it's already attached.")
        }
    }
    utils::install.packages(unlist(packages))
    TRUE
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
    out <- list()

    for (pkg in pkgs) {
        out <- c(out, get_package_addins(pkg))
    }

    out
}

get_package_addins <- function(pkg) {
    path <- system.file("rstudio", "addins.dcf", package = pkg)
    if (!nzchar(path)) return(list())

    dcf <- tryCatch(read.dcf(path), error = function(cnd) {
        log_error(sprintf("Failed to read addins.dcf for '%s': %s", pkg, conditionMessage(cnd)))
        NULL
    })
    if (is.null(dcf)) return(list())

    cols <- colnames(dcf)
    out <- list()

    for (i in seq_len(nrow(dcf))) {
        addin <- parse_addin_row(dcf, i, cols, pkg)
        if (!is.null(addin)) {
            out <- c(out, list(addin))
        }
    }

    out
}

parse_addin_row <- function(dcf, row, cols, pkg) {
    binding <- dcf_field(dcf, row, "Binding", cols)
    if (!nzchar(binding)) return(NULL)

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
    out <- list()

    for (pkg in pkgs) {
        out <- c(out, get_package_templates(pkg))
    }

    out
}

get_package_templates <- function(pkg) {
    path <- system.file("rmarkdown", "templates", package = pkg)
    if (!nzchar(path)) return(list())

    dirs <- list.dirs(path, recursive = FALSE, full.names = TRUE)
    out <- list()

    for (dir in dirs) {
        template <- parse_template_dir(dir, pkg)
        if (!is.null(template)) {
            out <- c(out, list(template))
        }
    }

    out
}

parse_template_dir <- function(dir, pkg) {
    yaml_path <- file.path(dir, "template.yaml")
    if (!file.exists(yaml_path)) return(NULL)

    meta <- tryCatch(yaml::read_yaml(yaml_path), error = function(cnd) {
        log_error(sprintf("Failed to read template.yaml at '%s': %s", yaml_path, conditionMessage(cnd)))
        NULL
    })
    if (is.null(meta)) return(NULL)

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
