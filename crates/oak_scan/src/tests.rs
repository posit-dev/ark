mod library;
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

/// Mark `package_dir` as an installed package, so library listing accepts it
/// and the workspace walk prunes it (see `is_installed_package()`).
fn mark_installed(package_dir: &Path) {
    fs::create_dir_all(package_dir.join("Meta")).unwrap();
    fs::write(package_dir.join("Meta").join("package.rds"), "").unwrap();
}
