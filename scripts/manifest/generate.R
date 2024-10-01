#' Write out templated Windows Application Manifest files
#'
#' We use two files to embed an application manifest on Windows:
#' - `{crate}-manifest.rc`
#' - `{crate}.exe.manifest`
#'
#' And we generate these two files for both `harp` and `ark`.
#'
#' They are effectively the same, we just need to swap out the crate names,
#' so we use this script to write the files in a consistent manner.
write_manifest <- function(root, crate) {
  crate_exe <- glue::glue("{crate}.exe.manifest")

  dest_folder <- file.path(root, "crates", crate, "resources", "manifest")
  dest_path_rc <- file.path(dest_folder, glue::glue("{crate}-manifest.rc"))
  dest_path_manifest <- file.path(dest_folder, crate_exe)

  src_folder <- file.path(root, "scripts", "manifest")
  src_path_rc <- file.path(src_folder, "template.rc")
  src_path_manifest <- file.path(src_folder, "template.manifest")

  # Write `{crate}-manifest.rc`
  data <- list(name = crate_exe)
  text <- brio::read_file(src_path_rc)
  text <- whisker::whisker.render(text, data = data)
  brio::write_file(text, dest_path_rc)

  # Write `{crate}.exe.manifest`
  data <- list(name = crate)
  text <- brio::read_file(src_path_manifest)
  text <- whisker::whisker.render(text, data = data)
  brio::write_file(text, dest_path_manifest)

  invisible(NULL)
}

root <- here::here()
crates <- c("ark", "harp")

for (crate in crates) {
  write_manifest(root, crate)
}
