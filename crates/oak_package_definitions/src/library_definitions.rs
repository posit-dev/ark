use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;

use oak_package::package::Package;
use oak_sources::traits::PackageCache;

use crate::definitions::PackageDefinitions;

#[derive(Clone, Debug)]
pub struct LibraryDefinitions {
    /// Cache used for looking up package sources. `dyn` to allow easy swapping to
    /// `TestPackageCache` in test files.
    cache: Arc<dyn PackageCache>,

    definitions: Arc<RwLock<HashMap<String, Option<Arc<PackageDefinitions>>>>>,
}

impl LibraryDefinitions {
    pub fn new(r: PathBuf, library_paths: Vec<PathBuf>) -> anyhow::Result<Self> {
        let cache = Arc::new(oak_sources::PackageCache::new(r, library_paths)?);
        Ok(Self {
            cache,
            definitions: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Create a [LibraryDefinitions] with a custom cache, only used when
    /// testing with a `TestPackageCache`
    #[cfg(any(test, feature = "testing"))]
    pub fn from_cache(cache: Arc<dyn PackageCache>) -> Self {
        Self {
            cache,
            definitions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn get(&self, package: &Package) -> Option<Arc<PackageDefinitions>> {
        let name = &package.description().name;

        // Try to get from cache first (could be `None` if we already tried to
        // load a non-existent or broken package)
        if let Some(entry) = self.definitions.read().unwrap().get(name) {
            return entry.clone();
        }

        // Not cached, try to load
        let definitions = match self.load_package(package) {
            Ok(Some(definitions)) => Some(Arc::new(definitions)),
            Ok(None) => None,
            Err(err) => {
                log::error!("Can't load R package definitions: {err:?}");
                None
            },
        };

        self.definitions
            .write()
            .unwrap()
            .insert(name.clone(), definitions.clone());

        definitions
    }

    pub fn load_package(&self, package: &Package) -> anyhow::Result<Option<PackageDefinitions>> {
        // Try loading sources from the cache, this may take a moment if sources have to
        // be populated first!
        let Some(directory) = self.cache.get(&package.description().name) else {
            // No package sources
            return Ok(None);
        };

        let directory = directory.join("R");

        PackageDefinitions::load_from_directory(&directory, package.namespace()).map(Some)
    }
}
