# ---------------------------------------------------------------------------------------------
# Copyright (C) 2023 Posit Software, PBC. All rights reserved.
# ---------------------------------------------------------------------------------------------

# 2023-12-06
# At the time of writing, this script is for "manual" execution on Windows.
# For the moment, it is a one-time setup task for a specific R installation.
#
# You must be running R as an administrator to execute this script.
# If, for example, you do this via RStudio, make sure to launch RStudio as an administrator.

# Typically something like:
# "C:\\Program Files\\Microsoft Visual Studio\\2022\\Community\\VC\\Tools\\MSVC\\14.37.32822\\bin\\Hostx86\\x86"
# A number of the pieces are unstable (year, Community vs Enterprise, exact version)
# so we try and use `vswhere.exe` and `vsdevcmd.bat` to find the exact path

get_visual_studio_tools_directory <- function() {
  # First find `vswhere.exe`, which is supposedly always in the same spot
  vswhere <- file.path("C:", "Program Files (x86)", "Microsoft Visual Studio", "Installer")
  
  if (!dir.exists(vswhere)) {
    stop("Microsoft Visual Studio Installer folder does not exist.")
  }
  
  vswhere <- file.path(vswhere, "vswhere.exe")
  vswhere <- normalizePath(vswhere, mustWork = TRUE)
  vswhere <- shQuote(vswhere)
  vswhere <- paste(vswhere, "-prerelease -latest -property installationPath")
  
  # `vswhere` tells us where Microsoft Visual Studio lives
  visualstudio <- system(vswhere, intern = TRUE)
  
  if (!is.character(visualstudio) && length(visualstudio) != 1L && !is.na(visualstudio) && !dir.exists(visualstudio)) {
    stop("`vswhere` failed to find Microsoft Visual Studio")
  }
  
  # Next we navigate to `vsdevcmd.bat`, which also has a stable path, according
  # to https://github.com/microsoft/vswhere/wiki/Start-Developer-Command-Prompt
  vscmdbat <- file.path(visualstudio, "Common7", "Tools", "VsDevCmd.bat")
  vscmdbat <- normalizePath(vscmdbat, mustWork = TRUE)
  vscmdbat <- shQuote(vscmdbat)
  vscmdbat <- paste(vscmdbat, "-arch=amd64 -startdir=none -host_arch=amd64 -no_logo")
  
  where <- "where dumpbin.exe"
  
  # Running `VsDevCmd.bat` puts tools like `dumpbin.exe` and `link.exe` on the
  # PATH in the current command prompt, so we run that and then ask `where` to
  # find `dumpbin.exe` (finding `link.exe` also finds one from RTools).
  command <- paste(vscmdbat, "&&", where)
  dumpbin <- system(command, intern = TRUE)
  
  if (length(dumpbin) > 1L) {
    warning("Found multiple `dumpbin.exe`. Looking for one tied to Visual Studio.")
    dumpbin <- dumpbin[grepl("Microsoft Visual Studio", dumpbin)]
    
    if (length(dumpbin) > 1L) {
      warning("Still have multiple `dumpbin.exe`. Taking the first.")
      dumpbin <- dumpbin[[1L]]
    }
  }
  if (!is.character(dumpbin) && length(dumpbin) != 1L && !is.na(dumpbin) && !file.exists(dumpbin)) {
    stop("`where` failed to find `dumpbin.exe`.")
  }
  
  # Now just look up one level
  path <- normalizePath(file.path(dumpbin, ".."))
  
  path
}

# Get the Visual Studio tools directory where `dumpbin.exe` and `lib.exe` live
path <- get_visual_studio_tools_directory()

# Put the path containing the tools on the PATH.
Sys.setenv(PATH = paste(path, Sys.getenv("PATH"), sep = ";"))

# Find R DLLs.
dlls <- list.files(R.home("bin"), pattern = "dll$", full.names = TRUE)

message("Generating .lib files for DLLs in ", R.home("bin"))

# Generate corresponding 'lib' file for each DLL.
for (dll in dlls) {

  # Check to see if we've already generated our exports
  def <- sub("dll$", "def", dll)
  if (file.exists(def))
    next

  # Call it on R.dll to generate exports.
  command <- sprintf("dumpbin.exe /EXPORTS /NOLOGO %s", dll)
  message("> ", command)
  output <- system(paste(command), intern = TRUE)

  # Remove synonyms.
  output <- sub("=.*$", "", output)

  # Find start, end markers
  start <- grep("ordinal\\s+hint\\s+RVA\\s+name", output)
  end <- grep("^\\s*Summary\\s*$", output)
  contents <- output[start:(end - 1)]
  contents <- contents[nzchar(contents)]

  # Remove forwarded fields (not certain that this does anything)
  contents <- grep("forwarded to", contents, invert = TRUE, value = TRUE, fixed = TRUE)

  # Parse into a table
  tbl <- read.table(text = contents, header = TRUE, stringsAsFactors = FALSE)
  exports <- tbl$name

  # Sort and re-format exports
  exports <- sort(exports)
  exports <- c("EXPORTS", paste("\t", tbl$name, sep = ""))

  # Write the exports to a def file
  def <- sub("dll$", "def", dll)
  cat(exports, file = def, sep = "\n")

  # Call 'lib.exe' to generate the library file.
  outfile <- sub("dll$", "lib", dll)
  fmt <- "lib.exe /def:%s /out:%s /machine:%s"
  cmd <- sprintf(fmt, def, outfile, .Platform$r_arch)
  system(cmd)

}
