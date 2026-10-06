mod notebook;
mod packages;
mod scheduler;
mod sources;
mod stale;
mod tidy;
mod watch;
mod workspace;

use std::fs;
use std::path::Path;

fn is_case_insensitive(dir: &Path) -> bool {
    fs::write(dir.join("probe"), "").unwrap();
    let insensitive = dir.join("PROBE").exists();
    fs::remove_file(dir.join("probe")).unwrap();
    insensitive
}
