use std::path::Path;
use std::path::PathBuf;

use crate::directories;
use crate::write;
use crate::Status;

const SCRIPT: &str = include_str!("../scripts/srcrefs.R");

/// Returns the cached srcref source directory for a package if it exists.
pub fn get(package: &str, version: &str) -> anyhow::Result<Option<PathBuf>> {
    let path = directories::sources_cache_dir("srcref", package, version)?;

    if path.exists() {
        return Ok(Some(path));
    }

    Ok(None)
}

/// Extracts an R package's source files from srcref metadata, adds them to the
/// cache, and returns the cache path.
///
/// Launches a sidecar R session to read the srcrefs from the installed package.
pub fn add(
    package: &str,
    version: &str,
    rscript: &Path,
    libpaths: &[&str],
) -> anyhow::Result<Status> {
    let destination = directories::sources_cache_dir("srcref", package, version)?;

    // Already cached, we assume the cache is correct and just return immediately
    if destination.exists() {
        return Ok(Status::Success(destination));
    }

    let args = &[package, version];

    // To ensure the correct R package libraries are checked
    let libpaths = libpaths.join(":");
    let env = &[("R_LIBS", libpaths.as_str())];

    let output = aether_r_process::run_script(rscript, SCRIPT, args, env)?;

    let code = output.status.code().unwrap_or(1);

    // Exit code 2 means `Status::NotFound` (not installed, version mismatch, no srcrefs)
    if code == 2 {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::trace!("R script returned with exit code {code} for {package} {version}: {stderr}");
        return Ok(Status::NotFound);
    }

    // Any other unexpected failure
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
    write::write_to_cache(&files, &destination)?;

    Ok(Status::Success(destination))
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

    // #[test]
    // fn test_not_found() {
    //     let path = PathBuf::from("/usr/local/bin/Rscript");
    //     let libpaths = &[
    //         "/Users/davis/Library/R/arm64/4.5/library",
    //         "/Library/Frameworks/R.framework/Versions/4.5-arm64/Resources/library",
    //     ];
    //     let result = add("ellmer", "0.4.0.9000", path.as_path(), libpaths);
    //     assert!(matches!(result, Ok(Status::NotFound)));
    // }
}
