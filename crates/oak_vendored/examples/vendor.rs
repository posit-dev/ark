//! Regenerate the vendored base R sources archive
//!
//! Maintainer tool, run via `just vendor-r-sources`. Reads `vendor/versions.txt`,
//! downloads each listed R source tarball from CRAN, keeps only the `src/library/*/R/`
//! subtree of each, and writes a single solid `vendor/r-base-sources.tar.zst` in a highly
//! compressed format.
//!
//! To regenerate after a new version of R comes out, just add it to `vendor/versions.txt`
//! and rerun `just vendor-r-sources`.
//!
//! To maximally reduce duplication across R versions, we leverage zstd's compression
//! window feature. The compression algorithm "remembers" a window of previous data to
//! find and compress repeating patterns. Since there are very few changes between R
//! versions in the R sources, we can utilize this by placing `4.4.0/utils/R/help.R`
//! directly adjacent to `4.5.0/utils/R/help.R` in the tar file. This allows us to pack
//! the R sources for all supported patch releases of R into a single 1.5 MB `.tar.zst`
//! that is vendored with ark.
//!
//! We've pinned down some zstd options:
//!
//! - Compression level 19. Chosen because it is the maximum "normal" amount of
//!   compression. Compression time is paid once on our end, and decompression is fast for
//!   users.
//!
//! - Window log 23 (2^23 = 8 MB of lookback memory). This ends up being the default
//!   with compression level 19 and a large file, but we chose it on purpose. Some local
//!   testing showed that we don't need `--long` (implying window log 27, 128 MB) to
//!   maximize compression gains here. Going from 23 to 27 only compressed a further ~4%,
//!   which is great! It means that we can limit the memory required to decompress on the
//!   user's machine to 8 MB, while still getting maximal compression.
//!
//! The `.tar.zst` is also byte-reproducible across calls to `just vendor-r-sources` with
//! the same `versions.txt`. We accomplish this by normalizing all tar header data,
//! (including mtime, uid, gid, and file permissions) and having a fixed zstd compression
//! level and window log.

use std::io::Read;
use std::path::Path;
use std::time::Duration;

use flate2::read::GzDecoder;

/// zstd compression level
const ZSTD_LEVEL: i32 = 19;

/// zstd window log (2^23 = 8 MB of memory required to decompress)
///
/// This MUST match `ZSTD_WINDOW_LOG` that we decompress with in `extract.rs`
const ZSTD_WINDOW_LOG: u32 = 23;

/// One file kept from an R source tarball, ready to place in the solid archive
struct Entry {
    /// Version this file came from, as the original `x.y.z` string
    version: String,
    /// Version this file came from, parsed into (major, minor, patch) for ordering
    version_number: (u32, u32, u32),
    /// Path without the leading `{version}/`, e.g. `utils/R/help.R`
    path: String,
    /// File contents
    data: Vec<u8>,
}

fn main() -> anyhow::Result<()> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let vendor_dir = manifest_dir.join("vendor");

    let versions = read_versions(&vendor_dir.join("versions.txt"))?;

    // Collect `R/` files for each base R package across all supported R versions
    let mut entries = Vec::new();
    for version in &versions {
        eprintln!("Downloading R {version}");
        let tarball = download(version)?;
        collect_r_sources(version, tarball, &mut entries)?;
    }

    // Sort all entries by `(path, version_number)`, for example, `("utils/R/help.R",
    // "4.5.0")`. This places all versions of a given file adjacent to each other in the
    // tar file, allowing zstd's compression window feature to dedup identical copies
    // across versions.
    entries.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.version_number.cmp(&right.version_number))
    });

    let output = vendor_dir.join("r-base-sources.tar.zst");
    write_archive(&entries, &output)?;

    eprintln!(
        "Wrote {count} files from {versions} versions to {output}",
        count = entries.len(),
        versions = versions.len(),
        output = output.display()
    );

    Ok(())
}

/// Read `versions.txt`, one `x.y.z` per line
fn read_versions(path: &Path) -> anyhow::Result<Vec<String>> {
    let contents = std::fs::read_to_string(path)?;

    Ok(contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Parse `x.y.z` into a comparable tuple
fn parse_version(version: &str) -> Option<(u32, u32, u32)> {
    let mut parts = version.split('.');

    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;

    if parts.next().is_some() {
        return None;
    }

    Some((major, minor, patch))
}

/// Download the R source tarball for `version`, returning its gzipped bytes
fn download(version: &str) -> anyhow::Result<Vec<u8>> {
    /// CRAN mirrors to try, in order
    const MIRRORS: &[&str] = &["https://cran.rstudio.com", "https://cran.r-project.org"];

    const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
    const GLOBAL_TIMEOUT: Duration = Duration::from_secs(300);

    let major = version
        .split('.')
        .next()
        .ok_or_else(|| anyhow::anyhow!("Invalid R version: {version}"))?;

    let suffix = format!("src/base/R-{major}/R-{version}.tar.gz");

    let mut last_error = None;

    for mirror in MIRRORS {
        let url = format!("{mirror}/{suffix}");

        let request = ureq::get(&url)
            .config()
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(GLOBAL_TIMEOUT))
            .build();

        match request.call() {
            Ok(response) => {
                let mut bytes = Vec::new();
                response.into_body().into_reader().read_to_end(&mut bytes)?;
                return Ok(bytes);
            },
            Err(err) => {
                last_error = Some(err);
                continue;
            },
        }
    }

    Err(anyhow::anyhow!(
        "Failed to download R {version}: {err:?}",
        err = last_error.expect("`MIRRORS` is non-empty")
    ))
}

/// Extract the `src/library/*/R/` subtree from an R source tarball
///
/// The tarball wraps everything in a top-level `R-{version}/`. We keep only files under
/// `src/library/{package}/R/`, storing them with a `path` of `{package}/R/{file}.R` for
/// ordering purposes.
fn collect_r_sources(
    version: &str,
    tarball: Vec<u8>,
    entries: &mut Vec<Entry>,
) -> anyhow::Result<()> {
    let version_number =
        parse_version(version).ok_or_else(|| anyhow::anyhow!("Invalid R version: {version}"))?;

    let gz = GzDecoder::new(tarball.as_slice());
    let mut archive = tar::Archive::new(gz);

    for entry in archive.entries()? {
        let mut entry = entry?;

        if !entry.header().entry_type().is_file() {
            continue;
        }

        let path = entry.path()?.into_owned();
        let Some(path) = detect_package_r_file(&path) else {
            continue;
        };

        let mut data = Vec::new();
        entry.read_to_end(&mut data)?;

        entries.push(Entry {
            version_number,
            version: version.to_owned(),
            path,
            data,
        });
    }

    Ok(())
}

/// Detect files that live under `R-{version}/src/library/{package}/R/{rest}` and return
/// their destination path of `{package}/R/{rest}`
///
/// For some base packages, like parallel, `{rest}` can be another subfolder, like
/// `parallel/R/unix/{file}.R`.
///
/// Anything that doesn't live under that file path returns `None`
fn detect_package_r_file(path: &Path) -> Option<String> {
    let components: Vec<&str> = path
        .components()
        .map(|component| component.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()?;

    // R-{version} / src / library / {package} / R / <rest...>
    let [_r_version, "src", "library", package, "R", rest @ ..] = components.as_slice() else {
        return None;
    };

    // We need a file!
    if rest.is_empty() {
        return None;
    }

    let mut destination = format!("{package}/R");

    for part in rest {
        destination.push('/');
        destination.push_str(part);
    }

    Some(destination)
}

/// Write the sorted entries to a solid `tar.zst` archive reproducibly
fn write_archive(entries: &[Entry], output: &Path) -> anyhow::Result<()> {
    let file = std::fs::File::create(output)?;

    let mut encoder = zstd::stream::write::Encoder::new(file, ZSTD_LEVEL)?;
    encoder.set_parameter(zstd::zstd_safe::CParameter::WindowLog(ZSTD_WINDOW_LOG))?;

    let mut builder = tar::Builder::new(encoder);

    for entry in entries {
        let path = format!(
            "{version}/{path}",
            version = entry.version,
            path = entry.path
        );

        // Start with a blank GNU header (the default for the tar crate)
        let mut header = tar::Header::new_gnu();

        // Write reproducible header information
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(entry.data.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);

        // Finalize checksum
        header.set_cksum();

        // Write it!
        builder.append_data(&mut header, path, entry.data.as_slice())?;
    }

    let encoder = builder.into_inner()?;
    encoder.finish()?;

    Ok(())
}
