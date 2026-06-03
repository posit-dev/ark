use super::*;

fn url(s: &str) -> Url {
    Url::parse(s).unwrap()
}

#[test]
fn test_from_url_dispatches_by_scheme() {
    let file = FilePath::from_url(&url("file:///C:/Users/foo.R"));
    assert!(matches!(file, FilePath::File(_)));

    let ark = FilePath::from_url(&url("ark://namespace/foo.R"));
    assert!(matches!(ark, FilePath::Virtual(_)));

    let untitled = FilePath::from_url(&url("untitled:Untitled-1"));
    assert!(matches!(untitled, FilePath::Virtual(_)));
}

#[test]
fn test_virtual_preserves_bytes() {
    let original = url("ark://namespace/foo.R?cell=2#frag");
    let fp = FilePath::from_url(&original);
    assert_eq!(fp.to_url(), original);
}

#[test]
#[cfg(not(windows))]
fn test_abs_path_normalises_dot_dot() {
    let path = AbsPathBuf::from_utf8(Utf8PathBuf::from("/a/./b/../c")).unwrap();
    assert_eq!(path.as_path(), Utf8Path::new("/a/c"));
}

#[test]
#[cfg(not(windows))]
fn test_abs_path_collapses_repeated_separators() {
    let path = AbsPathBuf::from_utf8(Utf8PathBuf::from("/a//b///c")).unwrap();
    assert_eq!(path.as_path(), Utf8Path::new("/a/b/c"));
}

#[test]
#[cfg(not(windows))]
fn test_abs_path_strips_trailing_slash() {
    let path = AbsPathBuf::from_utf8(Utf8PathBuf::from("/a/b/")).unwrap();
    assert_eq!(path.as_path(), Utf8Path::new("/a/b"));
}

#[test]
fn test_abs_path_rejects_relative() {
    assert!(AbsPathBuf::from_utf8(Utf8PathBuf::from("a/b")).is_none());
}

#[test]
#[cfg(not(windows))]
fn test_abs_path_round_trips_through_url() {
    let path = AbsPathBuf::from_utf8(Utf8PathBuf::from("/home/user/foo.R")).unwrap();
    let url = path.to_url();
    let back = AbsPathBuf::from_url(&url).unwrap();
    assert_eq!(path, back);
}

#[test]
#[cfg(target_os = "macos")]
fn test_abs_path_does_not_resolve_symlinks() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let file = dir.path().join("test.R");
    std::fs::write(&file, "").unwrap();

    let path = AbsPathBuf::from_path(&file).unwrap();
    assert!(path.as_path().as_str().starts_with("/tmp/"));
}

// Windows-specific tests: synthesised paths exercise the drive
// uppercase logic. We use `Utf8PathBuf::from` directly so the tests
// run on Unix too; `from_path_buf` would reject Windows-shaped
// paths there.

#[test]
#[cfg(windows)]
fn test_abs_path_uppercases_drive_letter() {
    let path = AbsPathBuf::from_utf8(Utf8PathBuf::from("c:\\Users\\test")).unwrap();
    assert!(path.as_path().as_str().starts_with("C:"));
}

#[test]
#[cfg(windows)]
fn test_abs_path_preserves_uppercase_drive() {
    let path = AbsPathBuf::from_utf8(Utf8PathBuf::from("C:\\Users\\test")).unwrap();
    assert!(path.as_path().as_str().starts_with("C:"));
}

#[test]
fn test_uppercase_disk_prefix_lowercase() {
    assert_eq!(uppercase_disk_prefix("c:"), "C:");
    assert_eq!(uppercase_disk_prefix("\\\\?\\c:"), "\\\\?\\C:");
}

#[test]
fn test_uppercase_disk_prefix_already_upper() {
    assert_eq!(uppercase_disk_prefix("C:"), "C:");
}

#[test]
fn test_uppercase_disk_prefix_non_disk() {
    // UNC paths shouldn't be touched.
    assert_eq!(
        uppercase_disk_prefix("\\\\server\\share"),
        "\\\\server\\share"
    );
}
