//! Test and fuzz paths use native filesystem identities. Their databases supply
//! fixture contents without reading these paths from the host filesystem.

use aether_path::FilePath;
use camino::Utf8Component;
use camino::Utf8Path;

pub(crate) fn file_path(name: &str) -> FilePath {
    let root = if cfg!(windows) { "C:/" } else { "/" };
    let path = Utf8Path::new(root).join(name.trim_start_matches('/'));
    match FilePath::from_path_buf(path.into_std_path_buf()) {
        Some(path) => path,
        None => panic!("Invalid fixture path: {name}"),
    }
}

/// Render fixture names without a platform-specific root or URL escaping.
pub(crate) fn path_name(path: &FilePath) -> String {
    let Some(path) = path.as_path() else {
        panic!("Expected a filesystem fixture path: {path}");
    };
    path.components()
        .filter_map(|component| match component {
            Utf8Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_names_are_path_data() {
        for name in ["ws/a #?%.R", "abs/b.R"] {
            assert_eq!(path_name(&file_path(name)), name);
            assert_eq!(file_path(&format!("/{name}")), file_path(name));
        }
    }
}
