mod eviction;
mod identity;
mod notebook;
mod packages;
mod scheduler;
mod sources;
mod tidy;
mod watch;
mod workspace;

use std::fs;
use std::path::Path;

/// Callers skip their test when this returns `false`. The skip is printed so a
/// broken probe doesn't make those tests pass silently on every volume.
fn is_case_insensitive(dir: &Path) -> bool {
    fs::write(dir.join("probe"), "").unwrap();
    let insensitive = dir.join("PROBE").exists();
    fs::remove_file(dir.join("probe")).unwrap();
    if !insensitive {
        eprintln!(
            "Skipping: {dir} is on a case-sensitive volume",
            dir = dir.display()
        );
    }
    insensitive
}

/// Callers skip their test unless an NFC name resolves to a file created with
/// its NFD spelling. As in [`is_case_insensitive()`], the skip is printed so a
/// broken probe cannot silently skip all coverage.
fn is_normalization_insensitive(dir: &Path) -> bool {
    fs::write(dir.join("probe-e\u{301}"), "").unwrap();
    let insensitive = dir.join("probe-\u{e9}").exists();
    fs::remove_file(dir.join("probe-e\u{301}")).unwrap();
    if !insensitive {
        eprintln!(
            "Skipping: {dir} is on a normalization-sensitive volume",
            dir = dir.display()
        );
    }
    insensitive
}
