mod extract;

use std::cmp::Ordering;
use std::path::PathBuf;
use std::sync::LazyLock;

use oak_cache::Cache;

/// Cache version
const CACHE_VERSION: &str = "v1";

/// LRU capacity for cached vendored version trees, kept small because each is large
///
/// TODO!: Remove me after merging posit-dev/ark#1323
const CAPACITY: usize = 5;

/// Vendored R versions, sorted ascending
static VERSIONS: LazyLock<Vec<Version>> =
    LazyLock::new(|| read_versions(include_str!("../vendor/versions.txt")));

/// Solid zstd archive of every vendored version's base R sources
const ARCHIVE: &[u8] = include_bytes!("../vendor/r-base-sources.tar.zst");

/// Cache for vendored base R package sources
///
/// See `crates/oak_vendored/examples/vendor.rs` for archive design details
///
/// Base package sources are uncompressed from the embedded [`ARCHIVE`] and written to
/// the cache at `vendored/v1/r/{version}/`
///
/// The cache holds its shared root lock for the life of this `VendoredCache`, so any
/// path handed out stays valid as long as the `VendoredCache` lives.
#[derive(Debug)]
pub struct VendoredCache {
    cache: Cache,
}

impl VendoredCache {
    pub fn open() -> anyhow::Result<Self> {
        Ok(Self {
            cache: Cache::open(&format!("vendored/{CACHE_VERSION}/r"), CAPACITY)?,
        })
    }

    /// Like [`VendoredCache::open`], but rooted at an explicit `root` rather than the
    /// shared cache directory. Only useful for testing against a temp directory.
    pub fn open_in(root: PathBuf) -> anyhow::Result<Self> {
        Ok(Self {
            cache: Cache::open_in(root, CAPACITY)?,
        })
    }

    /// Get the cached path for `version` if already present
    pub fn get(&self, version: &str) -> Option<PathBuf> {
        let version = resolve_version(version)?;
        self.cache.get(version)
    }

    /// Populate and return the cached path for `version` from the embedded archive
    pub fn insert(&self, version: &str) -> Option<PathBuf> {
        let version = resolve_version(version)?;
        self.cache
            .insert(version, |dir| extract::populate(version, dir))
            .unwrap_or_else(|err| {
                log::error!("Failed to extract vendored R {version} sources: {err:?}");
                None
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Version {
    name: String,
    major: u32,
    minor: u32,
    patch: u32,
}

impl Version {
    fn parse(name: &str) -> Option<Self> {
        let mut parts = name.split('.');

        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;

        if parts.next().is_some() {
            return None;
        }

        // We need to own the `name`. In theory we'd make the caller pass a `name: String`
        // to move `to_owned()` to the call site, but this would feed up all the way
        // through `VendoredCache::get()` and `VendoredCache::insert()` and that didn't
        // feel worth it.
        let name = name.to_owned();

        Some(Self {
            name,
            major,
            minor,
            patch,
        })
    }

    /// Compare by version number
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch))
    }
}

/// Resolve requested R `version` to the closest match in [`VERSIONS`]
///
/// - If we have an exact match, we return that.
///
/// - If the requested `version` is newer than our newest version in [`VERSIONS`], we
///   cap to the newest version in [`VERSIONS`] so that people on R-devel can still get
///   mostly reliable base package information.
///
/// - If the requested `version` is older than our oldest version in [`VERSIONS`],
///   something is probably wrong, so we return `None`.
fn resolve_version(version: &str) -> Option<&'static str> {
    let version = Version::parse(version)?;

    // Look for exact match
    if let Some(version) = VERSIONS
        .iter()
        .find(|candidate| candidate.cmp(&version) == Ordering::Equal)
    {
        return Some(version.name.as_str());
    }

    // Return the newest version we have if applicable
    let newest_version = VERSIONS.last().expect("must have at least one version");
    if version.cmp(newest_version) == Ordering::Greater {
        Some(newest_version.name.as_str())
    } else {
        None
    }
}

/// Parse the `versions.txt` manifest, one `x.y.z` per non-blank line
fn read_versions(contents: &str) -> Vec<Version> {
    contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| match Version::parse(line) {
            Some(version) => version,
            None => panic!("Malformed version in `versions.txt`: {line}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use tempfile::TempDir;

    use crate::resolve_version;
    use crate::VendoredCache;
    use crate::ARCHIVE;
    use crate::VERSIONS;

    /// The newest vendored version, i.e. the last entry of `versions.txt`
    fn newest() -> &'static str {
        VERSIONS.last().unwrap().name.as_str()
    }

    #[test]
    fn test_resolve_exact() {
        assert_eq!(resolve_version("4.5.0"), Some("4.5.0"));
        assert_eq!(resolve_version(newest()), Some(newest()));
    }

    #[test]
    fn test_resolve_newer_than_newest_caps_at_newest() {
        assert_eq!(resolve_version("6.0.0"), Some(newest()));
    }

    #[test]
    fn test_resolve_below_floor_or_gap_is_none() {
        // Below the floor
        assert_eq!(resolve_version("4.1.0"), None);
        // A gap below the newest
        assert_eq!(resolve_version("4.3.5"), None);
    }

    #[test]
    fn test_insert_extracts_known_file() {
        let dir = TempDir::new().unwrap();
        let vendored = VendoredCache::open_in(dir.path().to_path_buf()).unwrap();

        let root = vendored.insert("4.5.0").unwrap();
        let help = root.join("utils").join("R").join("help.R");

        assert!(help.is_file());
        assert!(help.metadata().unwrap().permissions().readonly());
    }

    #[test]
    fn test_get_insert_get_round_trip() {
        let dir = TempDir::new().unwrap();
        let vendored = VendoredCache::open_in(dir.path().to_path_buf()).unwrap();

        // Miss
        assert_eq!(vendored.get("4.5.0"), None);

        // Insert
        let root = vendored.insert("4.5.0").unwrap();

        // Hit
        assert_eq!(vendored.get("4.5.0"), Some(root));
    }

    #[test]
    fn test_above_newest_versions_share_one_entry() {
        let dir = TempDir::new().unwrap();
        let vendored = VendoredCache::open_in(dir.path().to_path_buf()).unwrap();

        // Both resolve to the newest entry, so they extract to the same cache dir
        let first = vendored.insert("6.0.0").unwrap();
        let second = vendored.insert("9.9.9").unwrap();
        assert_eq!(first, second);

        // And that's the same entry as inserting the newest version directly
        assert_eq!(vendored.insert(newest()), Some(first));
    }

    #[test]
    fn test_versions_txt_matches_archive() {
        let versions_txt: BTreeSet<&str> = VERSIONS
            .iter()
            .map(|version| version.name.as_str())
            .collect();

        let archive = zstd::stream::read::Decoder::new(ARCHIVE).unwrap();
        let mut archive = tar::Archive::new(archive);

        let mut versions_archive = BTreeSet::new();
        for entry in archive.entries().unwrap() {
            let entry = entry.unwrap();
            let path = entry.path().unwrap().into_owned();
            let top = path.components().next().unwrap();
            versions_archive.insert(top.as_os_str().to_str().unwrap().to_owned());
        }
        let versions_archive: BTreeSet<&str> =
            versions_archive.iter().map(String::as_str).collect();

        assert_eq!(versions_txt, versions_archive);
    }
}
