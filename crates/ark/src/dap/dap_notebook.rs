//
// dap_notebook.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

// Shared helpers for mapping notebook cell code to temporary source files.
//
// Both `DapJupyterHandler` (which handles `dumpCell` / `debugInfo`) and the
// console REPL (which needs to look up breakpoints for executed cell code)
// must agree on how cell source code maps to file paths. This module
// centralises that logic.

use std::sync::LazyLock;

use amalthea::wire::execute_request::CodeLocation;
use amalthea::wire::execute_request::Position;
use url::Url;

const HASH_SEED: u32 = 0;
const TMP_FILE_SUFFIX: &str = ".r";

/// The temporary file prefix, cached at first access to avoid `TMPDIR`
/// instability. On macOS, R may unset `TMPDIR` during startup, causing
/// `std::env::temp_dir()` to return `/tmp` instead of the per-session
/// `/var/folders/.../T/` directory.
static TMP_FILE_PREFIX: LazyLock<String> = LazyLock::new(|| {
    let mut tmp_dir = std::env::temp_dir();
    let pid = std::process::id();
    tmp_dir.push(format!("ark-debug-{pid}"));
    // Trailing separator so the prefix can be concatenated directly with
    // the hash and suffix (e.g. `{prefix}{hash}.r`).
    format!("{}/", tmp_dir.display())
});

/// The temporary file prefix used for notebook debug source files.
///
/// Deterministic for a given process: `{tmp_dir}/ark-debug-{pid}/`.
/// Reported to the frontend via `debugInfo` so that the `PathEncoder`
/// on the client side can independently produce the same paths.
pub fn tmp_file_prefix() -> &'static str {
    &TMP_FILE_PREFIX
}

pub fn hash_seed() -> u32 {
    HASH_SEED
}

pub fn tmp_file_suffix() -> &'static str {
    TMP_FILE_SUFFIX
}

/// Compute the temporary source file path for a piece of cell code.
///
/// This produces the same path that `dumpCell` writes to, allowing the
/// console REPL to look up breakpoints for notebook cells without a
/// `code_location` in the `execute_request`.
pub fn notebook_source_path(code: &str) -> String {
    let prefix = tmp_file_prefix();
    let hash = murmur2(code.as_bytes(), HASH_SEED);
    format!("{}{hash}{TMP_FILE_SUFFIX}", prefix)
}

/// Synthesize a [`CodeLocation`] pointing to the notebook temp file for a
/// cell chunk.
///
/// The range spans the entire code starting at (0, 0). This gives
/// `annotate_input()` the file URI it needs for the `#line` directive and
/// breakpoint injection.
pub fn notebook_code_location(code: &str) -> Option<CodeLocation> {
    let path = notebook_source_path(code);
    let uri = Url::from_file_path(&path).ok()?;

    let lines: Vec<&str> = code.split('\n').collect();
    let last_line = lines.last().unwrap_or(&"");
    let end_line = if lines.is_empty() { 0 } else { lines.len() - 1 };

    Some(CodeLocation {
        uri,
        start: Position {
            line: 0,
            character: 0,
        },
        end: Position {
            line: end_line as u32,
            character: last_line.len() as u32,
        },
    })
}

/// MurmurHash2 implementation for computing deterministic temp file paths
/// from cell source code.
///
/// This must match the client-side `PathEncoder` in
/// `positron-runtime-debugger` so that the notebook location mapper can
/// correlate cell URIs with runtime source paths.
pub fn murmur2(data: &[u8], seed: u32) -> u32 {
    const M: u32 = 0x5bd1e995;
    const R: u32 = 24;

    let len = data.len();
    let mut h: u32 = seed ^ (len as u32);

    let mut i = 0;
    while i + 4 <= len {
        let mut k = u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        k = k.wrapping_mul(M);
        k ^= k >> R;
        k = k.wrapping_mul(M);

        h = h.wrapping_mul(M);
        h ^= k;

        i += 4;
    }

    let remaining = len - i;
    if remaining >= 3 {
        h ^= (data[i + 2] as u32) << 16;
    }
    if remaining >= 2 {
        h ^= (data[i + 1] as u32) << 8;
    }
    if remaining >= 1 {
        h ^= data[i] as u32;
        h = h.wrapping_mul(M);
    }

    h ^= h >> 13;
    h = h.wrapping_mul(M);
    h ^= h >> 15;

    h
}

#[cfg(test)]
mod tests {
    use super::murmur2;
    use super::notebook_source_path;
    use super::tmp_file_prefix;
    use super::HASH_SEED;
    use super::TMP_FILE_SUFFIX;

    #[test]
    fn test_murmur2_empty() {
        assert_eq!(murmur2(b"", 0), 0);
    }

    #[test]
    fn test_murmur2_deterministic() {
        let hash1 = murmur2(b"x <- 1 + 1\nprint(x)", 42);
        let hash2 = murmur2(b"x <- 1 + 1\nprint(x)", 42);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_murmur2_seed_varies() {
        let hash1 = murmur2(b"test", 0);
        let hash2 = murmur2(b"test", 1);
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_murmur2_content_varies() {
        let hash1 = murmur2(b"cell_a", 0);
        let hash2 = murmur2(b"cell_b", 0);
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_notebook_source_path_deterministic() {
        let code = "x <- 1 + 1\nprint(x)";
        let path1 = notebook_source_path(code);
        let path2 = notebook_source_path(code);
        assert_eq!(path1, path2);
    }

    #[test]
    fn test_notebook_source_path_format() {
        let code = "print(1)";
        let path = notebook_source_path(code);
        let prefix = tmp_file_prefix();
        let hash = murmur2(code.as_bytes(), HASH_SEED);
        assert_eq!(path, format!("{prefix}{hash}{TMP_FILE_SUFFIX}"));
    }
}
