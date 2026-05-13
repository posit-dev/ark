use std::collections::HashMap;

use aether_url::UrlId;
use oak_semantic::semantic_index::DefinitionKind;
use oak_semantic::semantic_index::ScopeId;

use crate::Db;
use crate::File;
use crate::Script;

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
    /// here, but its definition lives in `script` under `name`.
    Import { script: Script, name: String },
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
    /// A walk over `semantic_index().definitions[file_scope]`, in
    /// source order. The walk respects R's "last assignment wins"
    /// semantics because the per-file index already merges locals and
    /// `source()`-injected Imports into the arena in source order.
    /// `HashMap::insert` then overwrites per name during the walk.
    ///
    /// `DefinitionKind::Import { file, name }` maps to
    /// `ExportEntry::Import { script, name }` via
    /// `SourceGraph::script_by_url`. If the target script isn't in
    /// the source graph (yet), the Import is dropped silently. The
    /// expected outcome since `DbResolver` only injects Imports when
    /// `script_by_url` resolves the target.
    ///
    /// `cycle_result` (FallbackImmediate) recovers from cyclic
    /// `source()` chains by returning empty exports for every cycle
    /// participant. R doesn't allow `A` sources `B` sources `A`, so
    /// precision loss is acceptable. `File::semantic_index` carries
    /// its own `cycle_result` for the same reason: salsa requires a
    /// handler on the first re-entered query, and a direct call to
    /// `semantic_index` outside `exports` would otherwise panic.
    #[salsa::tracked(returns(ref), cycle_result = exports_cycle_result)]
    pub fn exports(self, db: &dyn Db) -> FileExports {
        let index = self.semantic_index(db);
        let file_scope = ScopeId::from(0);
        let symbols = index.symbols(file_scope);
        let source_graph = db.source_graph();

        let mut entries: HashMap<String, ExportEntry> = HashMap::new();
        for (_id, def) in index.definitions(file_scope).iter() {
            let name = symbols.symbol(def.symbol()).name();
            let entry = match def.kind() {
                DefinitionKind::Import {
                    file: target_url,
                    name: target_name,
                    ..
                } => {
                    let target_url_id = UrlId::from_canonical(target_url.clone());
                    let Some(target_script) = source_graph.script_by_url(db, &target_url_id) else {
                        continue;
                    };
                    ExportEntry::Import {
                        script: target_script,
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
