use std::path::Path;
use std::path::PathBuf;

use tempfile::TempDir;

impl crate::traits::PackageCache for TestPackageCache {
    fn get(&self, package: &str) -> Option<PathBuf> {
        self.get(package)
    }
}

/// A fake package cache that can be used for testing
#[derive(Debug)]
pub struct TestPackageCache {
    root: TempDir,
}

impl TestPackageCache {
    pub fn new() -> anyhow::Result<Self> {
        let root = TempDir::new()?;
        Ok(Self { root })
    }

    pub fn get(&self, package: &str) -> Option<PathBuf> {
        let package = self.root.path().join(package);

        if package.exists() {
            Some(package)
        } else {
            None
        }
    }

    pub fn add(&self, package: &str, files: Vec<(&Path, &str)>) -> anyhow::Result<()> {
        let package = self.root.path().join(package);
        std::fs::create_dir(&package)?;

        let r = package.join("R");
        std::fs::create_dir(&r)?;

        for (name, content) in files {
            let path = r.join(name);
            std::fs::write(path, content)?;
        }

        Ok(())
    }
}
