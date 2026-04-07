use std::path::PathBuf;

pub fn aether_cache_dir() -> anyhow::Result<PathBuf> {
    Ok(xdg::BaseDirectories::with_prefix("aether")?.get_cache_home())
}

pub fn sources_cache_dir(cache: &str, package: &str, version: &str) -> anyhow::Result<PathBuf> {
    Ok(aether_cache_dir()?.join(cache).join(package).join(version))
}
