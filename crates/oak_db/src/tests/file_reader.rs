use std::fs;

use aether_path::FilePath;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use oak_package_metadata::index::Index;
use oak_package_metadata::namespace::Namespace;
use salsa::Setter;

use crate::file_reader::EmptyFileReader;
use crate::tests::test_db::TestDb;
use crate::Db;
use crate::File;
use crate::FileRevision;
use crate::OakDatabase;
use crate::Package;

#[test]
fn fixture_readers_ignore_existing_host_files() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let dir =
        Utf8Path::from_path(temp.path()).ok_or_else(|| anyhow::anyhow!("Non-UTF-8 temp path"))?;
    let files = fixture_contents(dir);
    for (path, text) in &files {
        fs::write(path, text)?;
    }

    // Establish that all four host files are readable and valid.
    assert_query_contents(&mut OakDatabase::new(), dir, true)?;
    assert_query_contents(&mut TestDb::new(), dir, false)?;

    // The empty reader must also survive cloning into a background snapshot.
    let db = OakDatabase::with_file_reader(EmptyFileReader);
    let mut snapshot = db.snapshot();
    drop(db);
    assert_query_contents(&mut snapshot, dir, false)?;
    Ok(())
}

#[test]
fn queries_read_in_memory_files() -> anyhow::Result<()> {
    let dir = crate::test_path::file_path("pkg");
    let dir = dir
        .as_path()
        .ok_or_else(|| anyhow::anyhow!("Expected filesystem path"))?;
    let mut db = TestDb::with_files(fixture_contents(dir));
    assert_query_contents(&mut db, dir, true)
}

fn fixture_contents(dir: &Utf8Path) -> [(Utf8PathBuf, String); 4] {
    [
        (dir.join("a.R"), "x <- 1\n".to_string()),
        (dir.join("NAMESPACE"), "export(x)\n".to_string()),
        (
            dir.join("DESCRIPTION"),
            "Package: pkg\nVersion: 1.0.0\n".to_string(),
        ),
        (dir.join("INDEX"), "x    A dataset\n".to_string()),
    ]
}

fn assert_query_contents(db: &mut impl Db, dir: &Utf8Path, present: bool) -> anyhow::Result<()> {
    let source_path = FilePath::from_path_buf(dir.join("a.R").into_std_path_buf())
        .ok_or_else(|| anyhow::anyhow!("Expected absolute source path"))?;
    let description_path = FilePath::from_path_buf(dir.join("DESCRIPTION").into_std_path_buf())
        .ok_or_else(|| anyhow::anyhow!("Expected absolute DESCRIPTION path"))?;
    let file = File::new(
        db,
        source_path,
        FileRevision::from(1u128),
        Some("editor text".to_string()),
        None,
    );
    assert_eq!(file.source_text(db), "editor text");
    file.set_source_text_override(db).to(None);
    assert_eq!(file.source_text(db), if present { "x <- 1\n" } else { "" });

    let overridden_namespace = Namespace::parse("export(editor)")?;
    let package = Package::new(
        db,
        description_path,
        "pkg".to_string(),
        FileRevision::from(1u128),
        FileRevision::from(1u128),
        Some(FileRevision::from(1u128)),
        Some(overridden_namespace.clone()),
        vec![],
        vec![],
    );
    assert_eq!(package.namespace(db), &overridden_namespace);
    package.set_namespace_override(db).to(None);
    let namespace = if present {
        Namespace::parse("export(x)")?
    } else {
        Namespace::default()
    };
    assert_eq!(package.namespace(db), &namespace);
    assert_eq!(
        package.version(db).as_deref(),
        if present { Some("1.0.0") } else { None }
    );
    let index = if present {
        Index::parse("x    A dataset\n")
    } else {
        Index::default()
    };
    assert_eq!(package.index(db), &Some(index));
    Ok(())
}
