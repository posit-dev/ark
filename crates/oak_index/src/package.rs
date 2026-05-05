use std::fs;
use std::path::Path;
use std::path::PathBuf;

use oak_package_metadata::description::Description;
use oak_package_metadata::index::Index;
use oak_package_metadata::namespace::Namespace;
use stdext::SortedVec;

/// Represents an R package and its metadata relevant for static analysis.
#[derive(Clone, Debug)]
pub struct Package {
    /// Path to the directory that contains `DESCRIPTION`. Can
    /// be an installed package or a package source.
    path: PathBuf,

    description: Description,
    namespace: Namespace,

    // List of symbols exported via NAMESPACE `export()` directives and via
    // documented symbols listed in INDEX. The latter is a stopgap to ensure we
    // support exported datasets and prevent spurious diagnostics (we accept
    // false negatives to avoid annoying false positives).
    exported_symbols: SortedVec<String>,
}

impl Package {
    pub fn new(
        path: PathBuf,
        description: Description,
        namespace: Namespace,
        index: Index,
    ) -> Self {
        // Compute exported symbols. Start from explicit NAMESPACE exports.
        let mut symbols = namespace.exports.clone().to_vec();

        // Add all documented symbols. This should cover documented datasets.
        symbols.extend(index.names.clone());

        let exported_symbols = SortedVec::from_vec(symbols);

        Self {
            path,
            description,
            namespace,
            exported_symbols,
        }
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn from_parts(path: PathBuf, description: Description, namespace: Namespace) -> Self {
        Self::new(path, description, namespace, Index::default())
    }

    /// Load a package from a given path.
    pub fn load_from_folder(package_path: &std::path::Path) -> anyhow::Result<Option<Self>> {
        let description_path = package_path.join("DESCRIPTION");

        // Only consider directories that contain a description file
        if !description_path.is_file() {
            return Ok(None);
        }

        // This fails if there is no `Package` field, so we're never loading
        // folders like bookdown projects as package
        let description_contents = fs::read_to_string(&description_path)?;
        let description = Description::parse(&description_contents)?;

        let namespace_path = package_path.join("NAMESPACE");
        let namespace = if namespace_path.is_file() {
            let namespace_contents = fs::read_to_string(&namespace_path)?;
            Namespace::parse(&namespace_contents)?
        } else {
            log::info!(
                "Package `{name}` doesn't contain a NAMESPACE file, using defaults",
                name = description.name
            );
            Namespace::default()
        };

        let index = match Index::load_from_folder(package_path) {
            Ok(index) => index,
            Err(err) => {
                log::warn!(
                    "Can't load INDEX file from `{path}`: {err:?}",
                    path = package_path.to_string_lossy()
                );
                Index::default()
            },
        };

        Ok(Some(Self::new(
            package_path.to_path_buf(),
            description,
            namespace,
            index,
        )))
    }

    /// Load a package from the given library path and name.
    pub fn load_from_library(
        lib_path: &std::path::Path,
        name: &str,
    ) -> anyhow::Result<Option<Self>> {
        let package_path = lib_path.join(name);

        // For library packages, ensure the invariant that the package name
        // matches the folder name
        if let Some(pkg) = Self::load_from_folder(&package_path)? {
            if pkg.description.name != name {
                return Err(anyhow::anyhow!(
                    "`Package` field in `DESCRIPTION` doesn't match folder name '{name}'"
                ));
            }

            Ok(Some(pkg))
        } else {
            Ok(None)
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn description(&self) -> &Description {
        &self.description
    }

    pub fn namespace(&self) -> &Namespace {
        &self.namespace
    }

    pub fn exported_symbols(&self) -> &SortedVec<String> {
        &self.exported_symbols
    }
}

#[cfg(any(test, feature = "testing"))]
pub fn temp_palmerpenguin() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();

    // Write DESCRIPTION
    let description = "\
Package: penguins
Version: 1.0
";
    fs::write(dir.path().join("DESCRIPTION"), description).unwrap();

    // Write NAMESPACE
    let namespace = "\
export(path_to_file)
export(penguins)
";
    fs::write(dir.path().join("NAMESPACE"), namespace).unwrap();

    // Write INDEX
    let index = "\
path_to_file            Get file path to 'penguins.csv' and
                    'penguins_raw.csv' files
penguins                Size measurements for adult foraging penguins
                    near Palmer Station, Antarctica
penguins_raw            Penguin size, clutch, and blood isotope data
                    for foraging adults near Palmer Station,
                    Antarctica
";
    fs::write(dir.path().join("INDEX"), index).unwrap();

    dir
}

#[cfg(test)]
mod tests {
    use oak_package_metadata::description::Description;
    use oak_package_metadata::index::Index;
    use oak_package_metadata::namespace::Namespace;

    use super::*;

    fn new_package(name: &str, ns: Namespace, index: Index) -> Package {
        Package::new(
            std::path::PathBuf::from("/fake"),
            Description {
                name: name.to_string(),
                ..Description::default()
            },
            ns,
            index,
        )
    }

    #[test]
    fn exported_symbols_are_sorted_and_unique() {
        let ns = Namespace {
            exports: SortedVec::from_vec(vec!["b".to_string(), "a".to_string(), "a".to_string()]),
            ..Default::default()
        };

        let index = Index {
            names: vec!["c".to_string(), "a".to_string(), "a".to_string()],
        };

        let pkg = new_package("foo", ns, index);
        assert_eq!(&*pkg.exported_symbols, &["a", "b", "c"]);
    }

    #[test]
    fn exported_symbols_empty_when_none() {
        let ns = Namespace::default();
        let idx = Index::default();
        let pkg = new_package("foo", ns, idx);
        assert!(pkg.exported_symbols.is_empty());
    }

    #[test]
    fn load_from_folder_reads_description_namespace_and_index() {
        let dir = temp_palmerpenguin();

        let pkg = Package::load_from_folder(dir.path()).unwrap().unwrap();

        // Should include all exports and all index names, sorted and deduped
        assert_eq!(&*pkg.exported_symbols, &[
            "path_to_file",
            "penguins",
            "penguins_raw"
        ]);
        assert_eq!(pkg.description.name, "penguins");
    }
}
