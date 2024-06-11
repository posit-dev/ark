#---------------------------------------------------------------------------------------------
# Copyright (C) 2024 Posit Software, PBC. All rights reserved.
#--------------------------------------------------------------------------------------------*/

# This utility generates a NOTICE file for a crate automatically, by
# looking up license for each of its top-level dependencies.
#
# Requires the cargo-license utility; to install that, run:
#
# $ cargo install cargo-license
#
# To use this script, run it in the top-level directory of the crate for which
# a NOTICE file is to be generated.

library(httr)
library(jsonlite)

# For TOML parsing
library(blogdown)

SPDX_LICENSE_BASE_URL <- "https://raw.githubusercontent.com/spdx/license-list-data/v3.20/text/"

# Function to fetch SPDX license text
fetch_spdx_license_text <- function(license) {
  url <- paste0(SPDX_LICENSE_BASE_URL, license, ".txt")
  response <- GET(url)

  if (status_code(response) == 200) {
    return(content(response, "text", encoding = "UTF-8"))
  } else {
    return(NULL)
  }
}

# Function to fetch license text from GitHub
fetch_github_license_text <- function(repo_url, license) {
  # Normalize GitHub URL
  if (grepl("github.com", repo_url)) {
    repo_url <- gsub("github.com", "raw.githubusercontent.com", repo_url)
    repo_url <- gsub("(/tree/|/blob/)", "/", repo_url)
    repo_url <- gsub("(/pull/|/issues/|/commit/|/releases/|/compare/|/actions/|/projects/|/discussions/|/wiki/|/network/|/security/|/settings/|/packages/|/pulse/|/community/|/code/)", "/", repo_url)

    # Check for LICENSE files
    license_file_url <- paste0(repo_url, "/master/LICENSE")
    response <- GET(license_file_url)
    if (status_code(response) == 200) {
      return(content(response, "text", encoding = "UTF-8"))
    } else {
      # Check for dual-license files
      license_file_url <- paste0(repo_url, "/master/LICENSE-", license)
      response <- GET(license_file_url)
      if (status_code(response) == 200) {
        return(content(response, "text", encoding = "UTF-8"))
      }
    }
  }
  return(NULL)
}

# Function to process a single package and append license info to NOTICE file
process_package <- function(package, base_path, notice_file) {
  message("Processing license for ", package$name)
  cat("\n---\n", file = notice_file, append = TRUE)
  cat("Package: ", package$name, "\n", file = notice_file, append = TRUE)
  cat("Version: ", package$version, "\n", file = notice_file, append = TRUE)

  licenses <- if (!is.null(package$license)) strsplit(package$license, " OR | AND ")[[1]] else NULL
  chosen_license <- if (!is.null(licenses) && "MIT" %in% licenses) "MIT" else licenses[1]

  if (!is.null(package$repository)) {
    license_text <- fetch_github_license_text(package$repository, chosen_license)
    if (!is.null(license_text)) {
      cat("License: ", chosen_license, "\n\n", file = notice_file, append = TRUE)
      cat(license_text, "\n", file = notice_file, append = TRUE)
      return()
    }
  }

  if (!is.null(chosen_license)) {
    cat("License: ", chosen_license, "\n\n", file = notice_file, append = TRUE)
    license_text <- fetch_spdx_license_text(chosen_license)
    if (!is.null(license_text)) {
      cat(license_text, "\n", file = notice_file, append = TRUE)
    }
  } else if (!is.null(package$license_file)) {
    cat("License File: ", package$license_file, "\n\n", file = notice_file, append = TRUE)
    license_path <- file.path(base_path, package$license_file)
    if (file.exists(license_path)) {
      license_text <- readLines(license_path, warn = FALSE)
      cat(paste(license_text, collapse = "\n"), "\n", file = notice_file, append = TRUE)
    }
  } else {
    cat("License: None\n\n", file = notice_file, append = TRUE)
  }
}

# Read Cargo.toml to get top-level dependencies
cargo_toml <- blogdown::read_toml("Cargo.toml")

# Run `cargo license --json` and capture the output
licenses_json <- system2("cargo", args = c("license", "--json"), stdout = TRUE)

# Parse the JSON output
licenses <- fromJSON(paste(licenses_json, collapse = "\n"))

# Get dependencies
dependencies <- cargo_toml$dependencies
if (is.null(dependencies)) {
  dependencies <- list()
}

# Create NOTICE file
notice_file <- "NOTICE"
file.create(notice_file)

cat("Amalthea R Kernel (ark) includes other open source software components.\n", file = notice_file, append = TRUE)
cat("The following is a list of each component and its license.\n\n", file = notice_file, append = TRUE)

# Process each dependency
for (package_name in names(dependencies)) {
  dep_info <- dependencies[[package_name]]

  # Handle different formats of dependency declarations
  if (is.character(dep_info)) {
    version <- dep_info
  } else if (is.list(dep_info)) {
    version <- dep_info$version
  } else {
    next
  }

  # Find the package in the licenses data
  package <- licenses[licenses$name == package_name & licenses$version == version, ]
  if (nrow(package) > 0) {
    process_package(package[1, ], dirname("Cargo.toml"), notice_file)
  }
}

cat("NOTICE file created successfully.\n")

