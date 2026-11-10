use std::io;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use yaml_rust2::Yaml;
use yaml_rust2::YamlLoader;

use crate::file_revision::report_untracked_if_zero;
use crate::Package;
use crate::RootKind;
use crate::SourceDb;

/// A roxygen tag described by a package's `roxygen2-tags.yml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoxygenTag {
    /// The tag without its `@`, e.g. `param`.
    pub name: String,
    /// Snippet inserted after the name, e.g. ` ${1:name} ${2:description}`.
    /// Its line breaks are bare, without the `#' ` prefix a roxygen comment
    /// needs on each line.
    pub template: Option<String>,
    /// Tag documentation in Markdown.
    pub description: Option<String>,
}

#[salsa::tracked]
impl Package {
    /// Read tag definitions from the `roxygen2-tags.yml` file shipped by
    /// roxygen2 for editor completions. Workspace packages use
    /// `inst/roxygen2-tags.yml`. Installed packages use `roxygen2-tags.yml`
    /// beside `DESCRIPTION`.
    ///
    /// A missing file gives no tags. Read errors, YAML parse errors and a
    /// top-level value that is not a list are logged and give no tags. Entries
    /// without a string-valued `name` are skipped.
    ///
    /// No revision tracks the tags file itself, so the query depends on
    /// `description_revision()`, which a reinstall bumps by rewriting
    /// `DESCRIPTION`. An edit to a workspace copy of the tags file is seen
    /// only after `DESCRIPTION` changes or a restart. While
    /// `description_revision()` is the zero sentinel, the query reruns on
    /// each database revision, as `Package::description()` does.
    ///
    /// Reads inputs and [`SourceDb::root_by_package()`], whose dependencies
    /// are also inputs. No dependency can call this query recursively, so it
    /// needs no cycle handler.
    #[salsa::tracked(returns(deref))]
    pub fn roxygen_tags(self, db: &dyn SourceDb) -> Vec<RoxygenTag> {
        report_untracked_if_zero(db, self.description_revision(db));

        let Some(path) = roxygen_tags_path(db, self) else {
            return Vec::new();
        };

        match db.read_to_string(&path) {
            Ok(text) => parse_roxygen_tags(&text, &path),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(err) => {
                log::error!("Failed to read `{path}`: {err:?}");
                Vec::new()
            },
        }
    }
}

fn roxygen_tags_path(db: &dyn SourceDb, package: Package) -> Option<Utf8PathBuf> {
    let dir = package.description_path(db).as_path()?.parent()?;
    let root = db.root_by_package(package)?;
    let path = match root.kind(db) {
        RootKind::Workspace => dir.join("inst").join("roxygen2-tags.yml"),
        RootKind::Library => dir.join("roxygen2-tags.yml"),
    };
    Some(path)
}

fn parse_roxygen_tags(text: &str, path: &Utf8Path) -> Vec<RoxygenTag> {
    let docs = match YamlLoader::load_from_str(text) {
        Ok(docs) => docs,
        Err(err) => {
            log::error!("Failed to parse `{path}`: {err:?}");
            return Vec::new();
        },
    };

    let Some(doc) = docs.first() else {
        return Vec::new();
    };
    let Some(entries) = doc.as_vec() else {
        log::error!("Expected a list of tags at the top level of `{path}`");
        return Vec::new();
    };

    entries.iter().filter_map(roxygen_tag).collect()
}

fn roxygen_tag(entry: &Yaml) -> Option<RoxygenTag> {
    // YAML indexing returns `BadValue` for non-mappings, so malformed entries
    // are skipped without a separate type check.
    let name = entry["name"].as_str()?;
    Some(RoxygenTag {
        name: name.to_string(),
        template: entry["template"].as_str().map(str::to_string),
        description: entry["description"].as_str().map(str::to_string),
    })
}
