//! Read installed packages' lazy data object names without loading the objects.
//!
//! `pkg::name` reaches a package's lazy data objects as well as its exports.
//! An installed package with lazy data has `data/Rdata.rds`, a named list from
//! dataset names to the names of the objects each dataset creates (`BJsales`
//! creates `BJsales` and `BJsales.lead`). Reading it needs R's deserializer, so
//! a sidecar R process reads every given file in one run.
//!
//! Unlike [`crate::primitives`], these names are not cached. In a measured
//! 687-package library, reading its 114 lazy data indexes took 19 ms and
//! produced 14 KB of names.

use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

const SCRIPT: &str = include_str!("../scripts/lazy_data.R");

/// Returns the lazy data index path for an installed package without checking
/// whether the file exists.
pub fn rdata_file(package_dir: &Path) -> PathBuf {
    package_dir.join("data").join("Rdata.rds")
}

/// Reads lazy data object names with the R executable `r`, in one process.
/// Returns one list per input file, in input order. An unreadable file or a
/// path that is not single-line UTF-8 gives an empty list. Names containing
/// tabs or line breaks are omitted because they cannot fit the output format.
/// Process or output errors fail the entire batch.
pub fn read_object_names(r: &Path, rdata_files: &[PathBuf]) -> anyhow::Result<Vec<Vec<String>>> {
    let mut names = vec![Vec::new(); rdata_files.len()];
    if rdata_files.is_empty() {
        return Ok(names);
    }

    let paths = write_paths_file(rdata_files)?;
    let paths_arg = paths.path().to_string_lossy();
    let output = oak_r_process::run_text(r, SCRIPT, &[paths_arg.as_ref()], &[])?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "Lazy data script failed with {status}: {stderr}",
            status = output.status
        ));
    }
    if !stderr.trim().is_empty() {
        log::warn!("Lazy data script reported: {stderr}");
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|err| anyhow::anyhow!("Lazy data script output was not valid UTF-8: {err}"))?;

    for line in stdout.lines() {
        let Some((index, name)) = line.split_once('\t') else {
            return Err(anyhow::anyhow!("Malformed lazy data script line: {line:?}"));
        };
        let Some(package_names) = index
            .parse::<usize>()
            .ok()
            .and_then(|index| names.get_mut(index))
        else {
            return Err(anyhow::anyhow!(
                "Lazy data script line has an invalid index: {line:?}"
            ));
        };
        package_names.push(name.to_string());
    }

    Ok(names)
}

/// Uses a file to avoid Windows' 32,767-character command-line limit when
/// hundreds of packages have lazy data. Paths that are not single-line UTF-8
/// become empty lines, preserving input indices while R reports a read error.
fn write_paths_file(rdata_files: &[PathBuf]) -> anyhow::Result<tempfile::NamedTempFile> {
    let mut file = tempfile::Builder::new()
        .suffix(".txt")
        .tempfile()
        .map_err(|err| anyhow::anyhow!("Failed to create temporary file: {err}"))?;

    for path in rdata_files {
        let line = match path.to_str() {
            Some(line) if !line.contains(['\n', '\r']) => line,
            _ => {
                log::warn!("Can't pass {path:?} to the lazy data script");
                ""
            },
        };
        writeln!(file, "{line}")
            .map_err(|err| anyhow::anyhow!("Failed to write temporary file: {err}"))?;
    }

    file.flush()
        .map_err(|err| anyhow::anyhow!("Failed to write temporary file: {err}"))?;

    Ok(file)
}
