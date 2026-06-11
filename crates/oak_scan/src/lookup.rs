//! Package lookup by `DESCRIPTION` URL.
//!
//! `oak_db` exposes `file_by_path` and `package_by_name` for analysis, but no
//! by-URL package lookup. Analysis never needs one: it keys packages by name
//! and is stale-blind. The scanner does need one, to find an existing
//! `Package` entity to reuse across rescans and eviction cycles. So the lookup
//! lives here on the write side, next to its only caller, rather than on
//! `oak_db`'s public `Db` trait.

use aether_path::FilePath;
use oak_db::Db;
use oak_db::LiveRoot;
use oak_db::Package;
use oak_db::Root;
use rustc_hash::FxHashMap;

use crate::stale::stale_package_path_index;

/// Find the `Package` registered at `path` (a `DESCRIPTION` URL), searching live
/// roots first and the [`oak_db::StaleRoot`] eviction bucket second.
///
/// Live roots are walked in lookup order (workspace then library) and the first
/// hit wins. A stale hit means the package's live container was dropped on an
/// earlier `set_*_paths` call. The scanner reuses that entity and moves it back
/// into a live container, which is why this lookup deliberately sees stale
/// packages where analysis (via `oak_db::Db::package_by_name`) does not.
pub(crate) fn package_by_path(db: &dyn Db, path: &FilePath) -> Option<Package> {
    for &root in db.live_roots() {
        if let LiveRoot::Workspace(r) | LiveRoot::Library(r) = root {
            if let Some(&pkg) = root_package_path_index(db, r).get(path) {
                return Some(pkg);
            }
        }
    }
    stale_package_path_index(db).get(path).copied()
}

/// Per-root DESCRIPTION URL -> Package index. Salsa caches one map per `Root`
/// and invalidates only when that root's packages change.
#[salsa::tracked(returns(ref))]
fn root_package_path_index(db: &dyn Db, root: Root) -> FxHashMap<FilePath, Package> {
    let mut map = FxHashMap::default();
    for &pkg in root.packages(db) {
        map.insert(pkg.description_path(db).clone(), pkg);
    }
    map
}
