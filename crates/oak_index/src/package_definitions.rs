use std::fs::DirEntry;
use std::path::Path;
use std::path::PathBuf;

use aether_parser::RParserOptions;
use biome_text_size::TextRange;
use oak_index_vec::define_index;
use oak_index_vec::IndexVec;
use oak_package_metadata::namespace::Namespace;
use rustc_hash::FxHashMap;
use stdext::result::ResultExt;

use crate::semantic_index;

define_index!(FileId);

/// The top level definitions exposed by a single package
#[derive(Debug)]
pub struct PackageDefinitions {
    /// Vector of file names indexable by `FileId`
    files: IndexVec<FileId, PathBuf>,

    /// Map from object name to its definition
    definitions: FxHashMap<String, PackageDefinition>,
}

impl PackageDefinitions {
    /// Given an `R/` directory, load all files recursively within that directory
    ///
    /// Most R packages have flat `R/` directories, but some base packages have OS
    /// specific subdirectories, notably utils and parallel.
    pub(crate) fn load_from_directory(
        directory: &Path,
        namespace: &Namespace,
    ) -> anyhow::Result<Self> {
        let mut files = IndexVec::new();
        let mut definitions = FxHashMap::default();

        visit_paths(
            directory,
            namespace,
            &mut files,
            &mut definitions,
            &append_definitions,
        )?;

        Ok(Self { files, definitions })
    }

    /// Get a [PackageDefinition] by name
    pub fn get(&self, name: &str) -> Option<&PackageDefinition> {
        self.definitions.get(name)
    }

    /// Get the [Path] for a [FileId]
    pub fn file(&self, file_id: FileId) -> &Path {
        self.files[file_id].as_path()
    }
}

/// For a given R file:
/// - Read from disk
/// - Parse
/// - Create a semantic index
/// - Extract top level file exports and categorize as exported/internal
fn append_definitions(
    entry: &DirEntry,
    namespace: &Namespace,
    files: &mut IndexVec<FileId, PathBuf>,
    definitions: &mut FxHashMap<String, PackageDefinition>,
) {
    let file = entry.path();

    if !oak_core::file::is_r_file(&file) {
        return;
    }

    let Some(content) = std::fs::read_to_string(&file).log_err() else {
        return;
    };

    let parsed = aether_parser::parse(&content, RParserOptions::default());
    if parsed.has_error() {
        return;
    }

    let index = semantic_index(&parsed.tree());

    let file_id = files.push(file);

    for (name, range) in index.file_exports() {
        let visibility = if namespace.exports.contains_str(name) {
            PackageDefinitionVisibility::Exported
        } else {
            PackageDefinitionVisibility::Internal
        };

        let definition = PackageDefinition {
            visibility,
            file_id,
            range,
        };

        definitions.insert(name.to_string(), definition);
    }
}

/// Recursively walk a directory
#[expect(clippy::type_complexity)]
fn visit_paths(
    directory: &Path,
    namespace: &Namespace,
    files: &mut IndexVec<FileId, PathBuf>,
    definitions: &mut FxHashMap<String, PackageDefinition>,
    cb: &dyn Fn(
        &DirEntry,
        &Namespace,
        &mut IndexVec<FileId, PathBuf>,
        &mut FxHashMap<String, PackageDefinition>,
    ),
) -> std::io::Result<()> {
    if directory.is_dir() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                visit_paths(&path, namespace, files, definitions, cb)?;
            } else {
                cb(&entry, namespace, files, definitions);
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PackageDefinitionVisibility {
    Exported,
    Internal,
}

/// A definition within a package
///
/// Retrieve its original file name using its [FileId] and [PackageDefinitions::file()].
#[derive(Debug)]
pub struct PackageDefinition {
    visibility: PackageDefinitionVisibility,
    file_id: FileId,
    range: TextRange,
}

impl PackageDefinition {
    pub fn visibility(&self) -> PackageDefinitionVisibility {
        self.visibility
    }

    pub fn file_id(&self) -> FileId {
        self.file_id
    }

    pub fn range(&self) -> TextRange {
        self.range
    }
}
