use std::path::Path;

use oak_fs::file_lock::FileLock;

const SCRIPT: &str = include_str!("../scripts/srcrefs.R");

/// Extracts an R package's source files from srcref metadata if possible and adds
/// them to the cache at the parent folder containing `destination_lock`
///
/// Launches a sidecar R session to read the srcrefs from the installed package.
pub(crate) fn cache_srcref<P: AsRef<Path>, Q: AsRef<Path>>(
    package: &str,
    version: &str,
    destination_lock: &FileLock,
    r: P,
    library_paths: &[Q],
) -> anyhow::Result<bool> {
    let args = &[package, version];

    // Set `R_LIBS` to ensure the correct R package libraries are checked
    // `:` on Unix, `;` on Windows, see `?R_LIBS`
    let library_paths = library_paths
        .iter()
        .map(|library_path| library_path.as_ref().to_string_lossy())
        .collect::<Vec<_>>()
        .join(if cfg!(windows) { ";" } else { ":" });
    let env = &[("R_LIBS", library_paths.as_str())];

    let output = oak_r_process::run_text(r.as_ref(), SCRIPT, args, env)?;

    let code = output.status.code().unwrap_or(1);

    // Exit code 2 means no srcrefs
    if code == 2 {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::trace!("R script returned with exit code {code} for {package} {version}: {stderr}");
        return Ok(false);
    }

    // Any other unexpected failure (unexpectedly not installed or wrong version)
    if code != 0 {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!(
            "R script failed with exit code {code} for {package} {version}: {stderr}"
        ));
    }

    let stdout = String::from_utf8(output.stdout).map_err(|err| {
        anyhow::anyhow!("R script output was not valid UTF-8 for {package} {version}: {err}")
    })?;

    let files = parse_output(&stdout);

    let destination = destination_lock.parent().join("R");
    std::fs::create_dir(&destination)?;

    for (name, contents) in files {
        let path = destination.join(name);
        std::fs::write(&path, contents)?;
        crate::fs::set_readonly(&path)?;
    }

    Ok(true)
}

/// Parses the concatenated srcref output into individual files.
///
/// The output format uses `#line 1 "<path>"` directives to separate files:
///
/// ```text
/// #line 1 "</install/path>/pkg/R/aaa.R"
/// <contents of aaa.R>
/// #line 1 "</install/path>/pkg/R/bbb.R"
/// <contents of bbb.R>
/// ```
fn parse_output(output: &str) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_lines: Vec<&str> = Vec::new();

    // This discards any lines before the first line directive, like this line, which
    // isn't part of the package sources `.packageName <- "vctrs"`
    for line in output.lines() {
        if let Some(name) = parse_line_directive(line) {
            // Flush the previous file
            if let Some(current_name) = current_name.take() {
                files.push((current_name, current_lines.join("\n")));
                current_lines.clear();
            }
            current_name = Some(name);
        } else {
            if current_name.is_some() {
                current_lines.push(line);
            }
        }
    }

    // Flush the last file
    if let Some(name) = current_name.take() {
        files.push((name, current_lines.join("\n")));
    }

    files
}

/// Extracts the filename from a line directive, keeping just the basename.
///
/// For example, `line 1 "</install/path>/pkg/R/aaa.R"` becomes `aaa.R`.
///
/// Does not have a way to fail on malformed line directives. If for some reason one is
/// malformed and doesn't have our expected prefix/suffix, then we would end up treating
/// it like a comment within a file.
fn parse_line_directive(line: &str) -> Option<String> {
    let path = line.strip_prefix("#line 1 \"")?;
    let path = path.strip_suffix('"')?;
    let file_name = Path::new(path).file_name()?;
    Some(file_name.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use oak_fs::file_lock::Filesystem;

    use super::*;

    #[test]
    fn test_parse_output_single_file() {
        let output = "\
#line 1 \"/path/to/pkg/R/aaa.R\"
fn_a <- function() {
  1 + 1
}";
        let files = parse_output(output);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "aaa.R");
        assert_eq!(files[0].1, "fn_a <- function() {\n  1 + 1\n}");
    }

    #[test]
    fn test_parse_output_multiple_files() {
        let output = "\
#line 1 \"/path/to/pkg/R/aaa.R\"
fn_a <- function() { }
#line 1 \"/path/to/pkg/R/bbb.R\"
fn_b <- function() { }";
        let files = parse_output(output);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].0, "aaa.R");
        assert_eq!(files[0].1, "fn_a <- function() { }");
        assert_eq!(files[1].0, "bbb.R");
        assert_eq!(files[1].1, "fn_b <- function() { }");
    }

    #[test]
    fn test_parse_output_leading_text() {
        let output = "\
packageName <- \"vctrs\"
#line 1 \"/path/to/pkg/R/aaa.R\"
fn_a <- function() {
  1 + 1
}";
        let files = parse_output(output);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "aaa.R");
        assert_eq!(files[0].1, "fn_a <- function() {\n  1 + 1\n}");
    }

    #[test]
    fn test_parse_output_empty() {
        let files = parse_output("");
        assert!(files.is_empty());
    }

    #[test]
    fn test_parse_output_no_directives() {
        let files = parse_output("just some text\nwith no directives");
        assert!(files.is_empty());
    }

    #[test]
    fn test_parse_line_directive() {
        assert_eq!(
            parse_line_directive("#line 1 \"/path/to/file.R\""),
            Some(String::from("file.R"))
        );
        assert_eq!(parse_line_directive("not a directive"), None);
        assert_eq!(parse_line_directive("#line 2 \"/path/to/file.R\""), None);
        assert_eq!(parse_line_directive("#line 1 missing-quotes"), None);
    }

    /// Requires R on the PATH and internet access
    ///
    /// Installs source version of {generics} from CRAN into a temporary library, then
    /// extracts the R source files via srcref metadata. We use {generics} because it
    /// is very easy to install from source and lightweight.
    #[test]
    fn test_srcref_extraction() {
        use std::process::Command;

        // Find R on PATH.
        // On Windows, `which` (from Git) returns POSIX paths that `Command::new()` can't
        // resolve. Use `where` which returns native paths.
        let output = Command::new(if cfg!(windows) { "where" } else { "which" })
            .arg("R")
            .output()
            .unwrap_or_else(|err| panic!("Failed to find R: {err}"));
        assert!(output.status.success());

        // Parse (`where` on Windows can return multiple matches, take the first)
        let r = PathBuf::from(
            String::from_utf8(output.stdout)
                .expect("Non-UTF8 R path")
                .trim()
                .lines()
                .next()
                .expect("R should exist"),
        );

        // Get base libpaths from R
        let output = oak_r_process::run_text(
            &r,
            r#"cat(normalizePath(.libPaths()), sep = "\n")"#,
            &[],
            &[],
        )
        .expect("Failed to get .libPaths()");
        assert!(output.status.success(), "Failed to query .libPaths()");

        let r_libpaths_original: Vec<PathBuf> = String::from_utf8(output.stdout)
            .expect("Non-UTF8 libpaths")
            .trim()
            .lines()
            .map(PathBuf::from)
            .collect();

        // Temporary library for installing generics
        let r_libpaths = tempfile::TempDir::new().unwrap();

        // Use forward slashes so the path is safe inside R string literals on Windows
        // (backslashes would be interpreted as escape sequences).
        let r_libpaths_for_interpolation =
            r_libpaths.path().display().to_string().replace('\\', "/");

        // Install generics from CRAN source with srcrefs preserved
        let output = oak_r_process::run_text(
            &r,
            &format!(
                    r#"install.packages("generics", lib = "{r_libpaths_for_interpolation}", repos = "https://cran.r-project.org", type = "source", INSTALL_opts = "--with-keep.source")"#,
                ),
            &[],
            &[],
        )
        .expect("Failed to run install.packages()");
        assert!(output.status.success());

        // Query the installed generics version
        let output = oak_r_process::run_text(
            &r,
            &format!(
                    r#"cat(as.character(packageVersion("generics", lib.loc = "{r_libpaths_for_interpolation}")))"#,
                ),
            &[],
            &[],
        )
        .expect("Failed to get generics version");
        assert!(output.status.success());

        let version = String::from_utf8(output.stdout)
            .expect("Non-UTF8 version")
            .trim()
            .to_string();

        // Prepend our temp library so generics is found there first
        let mut all_libpaths = vec![r_libpaths.path().to_path_buf()];
        all_libpaths.extend(r_libpaths_original);

        // Cache destination
        let destination_tempdir = tempfile::TempDir::new().unwrap();
        let destination = Filesystem::new(destination_tempdir.path().to_path_buf());
        let destination_lock = destination.open_rw_exclusive_create(".lock").unwrap();

        let ok = cache_srcref("generics", &version, &destination_lock, &r, &all_libpaths).unwrap();
        assert!(ok);

        // Verify R source files were written
        let r_dir = destination_lock.parent().join("R");
        assert!(r_dir.exists());

        let entries: Vec<_> = std::fs::read_dir(&r_dir)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(!entries.is_empty());

        // Verify files are readonly
        for entry in &entries {
            let metadata = entry.metadata().unwrap();
            assert!(metadata.permissions().readonly());
        }
    }
}
