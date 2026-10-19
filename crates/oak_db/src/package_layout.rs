//! Classifies package paths without reading the file, so callers can use the
//! same rule for packages that exist only in a workspace model.

use std::path::Path;

/// Direct `R/` children are loadable package files. Nested `R/` files are
/// skipped because R loads `R/` flat, while all other package paths are
/// analysed as scripts without package loading.
///
/// The scanner, watcher, and fuzz materializer share this rule to keep package
/// placement consistent.
#[derive(Debug, PartialEq)]
pub enum PackagePlacement {
    File,
    Script,
    Skip,
}

pub fn classify_in_package(package_dir: &Path, path: &Path) -> PackagePlacement {
    let r_dir = package_dir.join("R");
    if path.parent() == Some(r_dir.as_path()) {
        PackagePlacement::File
    } else if path.starts_with(&r_dir) {
        PackagePlacement::Skip
    } else {
        PackagePlacement::Script
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::classify_in_package;
    use super::PackagePlacement;

    #[test]
    fn classify_in_package_rule() {
        let pkg = Path::new("/ws/pkg");

        assert_eq!(
            classify_in_package(pkg, Path::new("/ws/pkg/R/a.R")),
            PackagePlacement::File
        );

        assert_eq!(
            classify_in_package(pkg, Path::new("/ws/pkg/R/sub/b.R")),
            PackagePlacement::Skip
        );

        assert_eq!(
            classify_in_package(pkg, Path::new("/ws/pkg/tests/testthat/test-a.R")),
            PackagePlacement::Script
        );
        assert_eq!(
            classify_in_package(pkg, Path::new("/ws/pkg/inst/foo.R")),
            PackagePlacement::Script
        );
        assert_eq!(
            classify_in_package(pkg, Path::new("/ws/pkg/data-raw/prep.R")),
            PackagePlacement::Script
        );
    }
}
