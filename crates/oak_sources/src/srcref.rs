use std::path::Path;
use std::path::PathBuf;

const SCRIPT: &str = include_str!("../scripts/srcrefs.R");

/// Extracts an R package's source files from srcref metadata if possible and adds
/// them to the cache at `destination`
///
/// Launches a sidecar R session to read the srcrefs from the installed package.
pub(crate) fn cache_srcref(
    package: &str,
    version: &str,
    destination: &Path,
    r_script_path: &Path,
    r_libpaths: &[PathBuf],
) -> anyhow::Result<bool> {
    let args = &[package, version];

    // Set `R_LIBS` to ensure the correct R package libraries are checked
    // `:` on Unix, `;` on Windows, see `?R_LIBS`
    let libpaths = r_libpaths
        .iter()
        .map(|libpath| libpath.to_string_lossy())
        .collect::<Vec<_>>()
        .join(if cfg!(windows) { ";" } else { ":" });
    let env = &[("R_LIBS", libpaths.as_str())];

    let script = write_script(SCRIPT)?;
    let output = oak_r_process::run_script(r_script_path, script.path(), args, env)?;

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

    let destination = destination.join("R");
    std::fs::create_dir(&destination)?;

    for (name, contents) in files {
        let path = destination.join(name);
        std::fs::write(&path, contents)?;
        crate::fs::set_readonly(&path)?;
    }

    Ok(true)
}

/// Writes a script string to a temporary file for execution by `Rscript`.
fn write_script(script: &str) -> anyhow::Result<tempfile::NamedTempFile> {
    use std::io::Write;
    let mut file = tempfile::Builder::new()
        .suffix(".R")
        .tempfile()
        .map_err(|err| anyhow::anyhow!("Failed to create temporary script file: {err}"))?;
    file.write_all(script.as_bytes())
        .map_err(|err| anyhow::anyhow!("Failed to write temporary script file: {err}"))?;
    Ok(file)
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

    /// Requires Rscript on PATH and internet access
    ///
    /// Installs source version of {generics} from CRAN into a temporary library, then
    /// extracts the R source files via srcref metadata. We use {generics} because it
    /// is very easy to install from source and lightweight.
    #[test]
    fn test_srcref_extraction() {
        use std::process::Command;

        // Find Rscript on PATH.
        // On Windows, `which` (from Git) returns POSIX paths that `Command::new()` can't
        // resolve. Use `where` which returns native paths.
        let output = Command::new(if cfg!(windows) { "where" } else { "which" })
            .arg("Rscript")
            .output()
            .unwrap_or_else(|err| panic!("Failed to find Rscript: {err}"));
        assert!(output.status.success());

        // Parse (`where` on Windows can return multiple matches, take the first)
        let r_script_path = PathBuf::from(
            String::from_utf8(output.stdout)
                .expect("Non-UTF8 Rscript path")
                .trim()
                .lines()
                .next()
                .expect("Rscript should exist"),
        );

        // Get base libpaths from this Rscript
        let output = Command::new(&r_script_path)
            .args([
                "--vanilla",
                "-e",
                r#"cat(normalizePath(.libPaths()), sep = "\n")"#,
            ])
            .output()
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
        let status = Command::new(&r_script_path)
            .args([
                "--vanilla",
                "-e",
                &format!(
                    r#"install.packages("generics", lib = "{r_libpaths_for_interpolation}", repos = "https://cran.r-project.org", type = "source", INSTALL_opts = "--with-keep.source")"#,
                ),
            ])
            .output()
            .expect("Failed to run install.packages()");
        assert!(status.status.success());

        // Query the installed generics version
        let output = Command::new(&r_script_path)
            .args([
                "--vanilla",
                "-e",
                &format!(
                    r#"cat(as.character(packageVersion("generics", lib.loc = "{r_libpaths_for_interpolation}")))"#,
                ),
            ])
            .output()
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
        let destination = tempfile::TempDir::new().unwrap();

        let ok = cache_srcref(
            "generics",
            &version,
            destination.path(),
            &r_script_path,
            &all_libpaths,
        )
        .unwrap();
        assert!(ok);

        // Verify R source files were written
        let r_dir = destination.path().join("R");
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
