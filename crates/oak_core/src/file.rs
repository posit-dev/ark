use std::path::Path;
use std::path::PathBuf;

/// Does this path's name look like an R file (`.R` / `.r` extension)?
///
/// Pure name test, no I/O. It doesn't touch the filesystem, so it says
/// nothing about whether the path exists or is a regular file. A
/// directory named `foo.R` passes, and so does a path that isn't on disk
/// at all. Callers that walk a real directory and want to skip such cases
/// must check `path.is_file()` themselves.
pub fn is_r_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("r"))
}

pub fn list_r_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| is_r_file(p))
        .collect()
}
