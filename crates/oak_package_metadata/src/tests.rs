//! Test-only fixtures shared across crates through the `testing` feature.

use std::fs;

use tempfile::TempDir;

/// Build a temporary library directory containing an installed `penguins`
/// package, with its `DESCRIPTION`, `NAMESPACE`, and `INDEX`. The package
/// lives in a directory named `penguins` so the library scanner discovers it
/// by directory name. Returns the library root.
pub fn temp_palmerpenguin() -> TempDir {
    let library = tempfile::tempdir().unwrap();
    let package = library.path().join("penguins");
    fs::create_dir(&package).unwrap();

    let description = "\
Package: penguins
Version: 1.0
";
    fs::write(package.join("DESCRIPTION"), description).unwrap();

    let namespace = "\
export(path_to_file)
export(penguins)
";
    fs::write(package.join("NAMESPACE"), namespace).unwrap();

    let index = "\
path_to_file            Get file path to 'penguins.csv' and
                    'penguins_raw.csv' files
penguins                Size measurements for adult foraging penguins
                    near Palmer Station, Antarctica
penguins_raw            Penguin size, clutch, and blood isotope data
                    for foraging adults near Palmer Station,
                    Antarctica
";
    fs::write(package.join("INDEX"), index).unwrap();

    library
}
