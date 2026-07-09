/// The default R search path packages always attached at startup
///
/// Listed in the order R's `search()` reports them (last attached = searched first, so
/// `stats` is highest-priority and `base` is lowest), which is important for when they
/// are attached as [crate::ImportLayer::Package] layers.
pub(crate) const DEFAULT_SEARCH_PATH_PACKAGES: [&str; 7] = [
    "stats",
    "graphics",
    "grDevices",
    "utils",
    "datasets",
    "methods",
    "base",
];
