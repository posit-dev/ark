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
    entries: HashMap<String, Vec<ExportEntry>>,
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
    /// Every entry bound to `name` in this file, deduplicated. A `Local`
    /// marker appears at most once even when the name has several top-level
    /// definitions; `File::resolve_export()` fans those back out by re-reading
    /// the semantic index. `Import` entries are distinct per `(file, name)`.
    pub fn get(&self, name: &str) -> Option<&[ExportEntry]> {
        self.entries.get(name).map(Vec::as_slice)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &[ExportEntry])> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v.as_slice()))
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
        let mut entries: HashMap<String, Vec<ExportEntry>> = HashMap::new();

        for (name, defs) in self.semantic_index(db).exports() {
            let list = entries.entry(name.to_string()).or_default();
            for (_def_id, def) in defs {
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
                // A name can have several live defs (e.g. both arms of an
                // `if`/`else`), and several can collapse to one marker: `Local`
                // and `Import` carry no `def_id`, so duplicates are byte-equal.
                // Dedup them, keeping definition order.
                if !list.contains(&entry) {
                    list.push(entry);
                }
            }
        }

        FileExports { entries }
    }
}

fn exports_cycle_result(_db: &dyn Db, _id: salsa::Id, _file: File) -> FileExports {
    FileExports::default()
}
