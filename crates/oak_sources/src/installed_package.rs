use std::fs::read_to_string;
use std::path::Path;
use std::path::PathBuf;

use oak_package::package_description::Description;

pub(crate) struct InstalledPackage {
    key: String,
    name: String,
    library_path: PathBuf,
    description: Description,
    description_hash: String,
}

impl InstalledPackage {
    pub(crate) fn find(package: &str, library_paths: &[PathBuf]) -> anyhow::Result<Option<Self>> {
        let mut library_path = None;

        for library_path_candidate in library_paths {
            if library_path_candidate.join(package).exists() {
                library_path = Some(library_path_candidate);
                break;
            }
        }

        let Some(library_path) = library_path else {
            // Not installed
            return Ok(None);
        };

        let package_path = library_path.join(package);

        let description_path = package_path.join("DESCRIPTION");
        let description_contents = read_to_string(&description_path)?;
        let description = Description::parse(&description_contents)?;

        let library_path_hash = crate::hash::hash(library_path.to_string_lossy().as_ref());
        let description_hash = crate::hash::hash(&description_contents);

        // Flat key unique enough to handle:
        // - The same R package across multiple libpaths
        // - Reinstalling a dev R package without changing the version (0.1.0.9000)
        let key = format!(
            "{name}_{version}_libpath-{library_path_hash}_description-{description_hash}",
            name = package,
            version = &description.version,
            library_path_hash = &library_path_hash,
            description_hash = &description_hash
        );

        Ok(Some(Self {
            key,
            name: package.to_string(),
            library_path: library_path.clone(),
            description,
            description_hash,
        }))
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn version(&self) -> &str {
        &self.description().version
    }

    pub(crate) fn description(&self) -> &Description {
        &self.description
    }

    // Flat key unique enough to handle:
    // - The same R package across multiple libpaths
    // - Reinstalling a dev R package without changing the version (0.1.0.9000)
    pub(crate) fn key(&self) -> &str {
        &self.key
    }

    pub(crate) fn library_path(&self) -> &Path {
        self.library_path.as_path()
    }

    pub(crate) fn package_path(&self) -> PathBuf {
        self.library_path.join(&self.name)
    }

    pub(crate) fn description_path(&self) -> PathBuf {
        self.package_path().join("DESCRIPTION")
    }

    pub(crate) fn namespace_path(&self) -> PathBuf {
        self.package_path().join("NAMESPACE")
    }

    pub(crate) fn description_hash(&self) -> &str {
        &self.description_hash
    }
}
