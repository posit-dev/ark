use aether_path::FilePath;
use camino::Utf8PathBuf;
use rustc_hash::FxHashMap;
use salsa::Setter;

use crate::file_reader::MapFileReader;
use crate::test_path::file_path;
use crate::DbInputs;
use crate::FileRevision;
use crate::OakDatabase;
use crate::Package;
use crate::PackageFiles;
use crate::Root;
use crate::RootKind;
use crate::RoxygenTag;

const TAGS: &str = r#"
- name: aliases
  description: >
    Add additional aliases to the topic.
  template: ' ${1:alias}'
  vignette: index-crossref
  recommend: true

- name: description
  template: "\n${1:A short description...}\n"

- name: export
"#;

fn tag(name: &str, template: Option<&str>, description: Option<&str>) -> RoxygenTag {
    RoxygenTag {
        name: name.to_string(),
        template: template.map(str::to_string),
        description: description.map(str::to_string),
    }
}

fn utf8_path(path: &FilePath) -> Utf8PathBuf {
    match path.as_path() {
        Some(path) => path.to_path_buf(),
        None => panic!("Expected a filesystem path: {path}"),
    }
}

/// Registers `roxygen2` with its `DESCRIPTION` at `{root_path}/roxygen2/`
/// on a root of `kind`.
fn register_roxygen2(db: &mut OakDatabase, root_path: FilePath, kind: RootKind) -> Package {
    let description_path = FilePath::from_path_buf(
        utf8_path(&root_path)
            .join("roxygen2")
            .join("DESCRIPTION")
            .into_std_path_buf(),
    )
    .unwrap();
    let package = Package::new(
        db,
        description_path,
        "roxygen2".to_string(),
        FileRevision::from(1u128),
        FileRevision::from(1u128),
        None,
        None,
        PackageFiles::default(),
        vec![],
    );
    let root = Root::new(db, root_path, kind, vec![], vec![package], vec![]);
    match kind {
        RootKind::Workspace => {
            db.workspace_roots().set_roots(db).to(vec![root]);
        },
        RootKind::Library => {
            db.library_roots().set_roots(db).to(vec![root]);
        },
    }
    package
}

/// An installed `roxygen2` under `lib/` whose tags file holds `tags`.
fn installed_roxygen2(tags: &str) -> (OakDatabase, Package) {
    let files = FxHashMap::from_iter([(
        utf8_path(&file_path("lib/roxygen2/roxygen2-tags.yml")),
        tags.to_string(),
    )]);
    let mut db = OakDatabase::with_file_reader(MapFileReader::new(files));
    let package = register_roxygen2(&mut db, file_path("lib"), RootKind::Library);
    (db, package)
}

#[test]
fn test_installed_package_tags() {
    let (db, package) = installed_roxygen2(TAGS);

    assert_eq!(package.roxygen_tags(&db), &[
        tag(
            "aliases",
            Some(" ${1:alias}"),
            Some("Add additional aliases to the topic.\n")
        ),
        tag("description", Some("\n${1:A short description...}\n"), None),
        tag("export", None, None),
    ]);
}

#[test]
fn test_workspace_package_reads_inst() {
    let files = FxHashMap::from_iter([
        (
            utf8_path(&file_path("ws/roxygen2/inst/roxygen2-tags.yml")),
            "- name: inst\n".to_string(),
        ),
        (
            utf8_path(&file_path("ws/roxygen2/roxygen2-tags.yml")),
            "- name: beside\n".to_string(),
        ),
    ]);
    let mut db = OakDatabase::with_file_reader(MapFileReader::new(files));
    let package = register_roxygen2(&mut db, file_path("ws"), RootKind::Workspace);

    assert_eq!(package.roxygen_tags(&db), &[tag("inst", None, None)]);
}

#[test]
fn test_missing_file_gives_no_tags() {
    let mut db = OakDatabase::with_file_reader(MapFileReader::new(FxHashMap::default()));
    let package = register_roxygen2(&mut db, file_path("lib"), RootKind::Library);

    assert_eq!(package.roxygen_tags(&db), &[]);
}

#[test]
fn test_package_outside_live_roots_gives_no_tags() {
    let files = FxHashMap::from_iter([(
        utf8_path(&file_path("lib/roxygen2/roxygen2-tags.yml")),
        TAGS.to_string(),
    )]);
    let db = OakDatabase::with_file_reader(MapFileReader::new(files));
    let package = Package::new(
        &db,
        file_path("lib/roxygen2/DESCRIPTION"),
        "roxygen2".to_string(),
        FileRevision::from(1u128),
        FileRevision::from(1u128),
        None,
        None,
        PackageFiles::default(),
        vec![],
    );

    assert_eq!(package.roxygen_tags(&db), &[]);
}

#[test]
fn test_malformed_yaml_gives_no_tags() {
    let (db, package) = installed_roxygen2("- name: aliases\n  description: [unclosed\n");

    assert_eq!(package.roxygen_tags(&db), &[]);
}

#[test]
fn test_top_level_not_a_list_gives_no_tags() {
    let (db, package) = installed_roxygen2("name: aliases\n");
    assert_eq!(package.roxygen_tags(&db), &[]);

    let (db, package) = installed_roxygen2("");
    assert_eq!(package.roxygen_tags(&db), &[]);
}

#[test]
fn test_entries_without_name_are_skipped() {
    let (db, package) = installed_roxygen2(
        "\
- description: No name
- name: 1
- just a string
- name: export
",
    );

    assert_eq!(package.roxygen_tags(&db), &[tag("export", None, None)]);
}

#[test]
fn test_description_revision_bump_rereads_file() {
    let library = tempfile::tempdir().unwrap();
    let tags_path = library.path().join("roxygen2").join("roxygen2-tags.yml");
    std::fs::create_dir_all(tags_path.parent().unwrap()).unwrap();
    std::fs::write(&tags_path, "- name: before\n").unwrap();

    let mut db = OakDatabase::new();
    let root_path = FilePath::from_path_buf(library.path().to_path_buf()).unwrap();
    let package = register_roxygen2(&mut db, root_path, RootKind::Library);
    assert_eq!(package.roxygen_tags(&db), &[tag("before", None, None)]);

    // Editing the tags file alone does not invalidate the cached result.
    std::fs::write(&tags_path, "- name: after\n").unwrap();
    assert_eq!(package.roxygen_tags(&db), &[tag("before", None, None)]);

    package
        .set_description_revision(&mut db)
        .to(FileRevision::from(2u128));
    assert_eq!(package.roxygen_tags(&db), &[tag("after", None, None)]);
}
