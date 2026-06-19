use std::collections::HashMap;

use aether_path::FilePath;
use oak_semantic::semantic_index::DefinitionKind;

use crate::Db;
use crate::File;

/// Names bound at top-level in a file.
///
/// When the file is sourced, these names and associated definitions get
/// injected in the calling scope.
///
/// Exports do not include source ranges so they are stable across
/// internal edits.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FileExports {
    entries: HashMap<String, ExportEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportEntry {
    /// A top-level definition local to this file.
    Local,
    /// A forwarding entry from a `source()` call. The name appears
    /// here, but its definition lives in `file` under `name`.
    Import { file: File, name: String },
}

impl FileExports {
    pub fn get(&self, name: &str) -> Option<&ExportEntry> {
        self.entries.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &ExportEntry)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[salsa::tracked]
impl File {
    /// Names this file exports.
    ///
    /// Delegates the walk to [`SemanticIndex::exports`], then translates
    /// each `DefinitionKind::Import { file, name }` into
    /// `ExportEntry::Import { file, name }` via [`Db::file_by_path`]. If
    /// the target file isn't interned yet, that Import is dropped
    /// silently. Expected, since [`SalsaImportsResolver`] only injects
    /// Imports when `file_by_path()` resolves the target.
    ///
    /// `cycle_result` (FallbackImmediate) recovers from cyclic `source()`
    /// chains by returning empty exports for every cycle participant.
    #[salsa::tracked(returns(ref), cycle_result = exports_cycle_result)]
    pub fn exports(self, db: &dyn Db) -> FileExports {
        let mut entries: HashMap<String, ExportEntry> = HashMap::new();
        for (name, (_def_id, def)) in self.semantic_index(db).exports() {
            let entry = match def.kind() {
                DefinitionKind::Import {
                    file: target_url,
                    name: target_name,
                    ..
                } => {
                    let target_url_id = FilePath::from_url(target_url);
                    let Some(target_file) = db.file_by_path(&target_url_id) else {
                        continue;
                    };
                    ExportEntry::Import {
                        file: target_file,
                        name: target_name.clone(),
                    }
                },
                _ => ExportEntry::Local,
            };
            entries.insert(name.to_string(), entry);
        }
        FileExports { entries }
    }
}

fn exports_cycle_result(_db: &dyn Db, _id: salsa::Id, _file: File) -> FileExports {
    FileExports::default()
}
