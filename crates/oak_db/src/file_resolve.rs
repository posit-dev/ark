use crate::Db;
use crate::Definition;
use crate::ExportEntry;
use crate::File;
use crate::Name;

#[salsa::tracked]
impl<'db> File {
    /// Resolve `name` against this file's exports.
    ///
    /// Chases `Import` forwarding entries (introduced by `source()`) through
    /// `exports(target_file)` until a `Local` is found. Returns `None` if the
    /// name isn't exported (or only chains through unresolved forwards /
    /// cycles, which `exports` recovers to empty).
    ///
    /// The returned `Definition` is keyed by `(file, scope, name)`, so
    /// downstream queries that only depend on identity stay cached across edits
    /// that shift the binding's source position. Consumers that need a position
    /// or range call the `def.range(db)` derived query.
    #[salsa::tracked]
    pub fn resolve(self, db: &'db dyn Db, name: Name<'db>) -> Option<Definition<'db>> {
        let mut current_file = self;
        let mut current_name = name.text(db).to_string();

        loop {
            let entry = current_file.exports(db).get(&current_name)?.clone();
            match entry {
                ExportEntry::Local => {
                    let range = local_definition_range(current_file, db, &current_name)?;
                    let file_scope = oak_semantic::semantic_index::ScopeId::from(0);
                    return Some(Definition::new(
                        db,
                        current_file,
                        file_scope,
                        Name::new(db, current_name.as_str()),
                        range,
                    ));
                },
                ExportEntry::Import { file, name } => {
                    current_file = file;
                    current_name = name;
                },
            }
        }
    }
}

/// Locate the range of a top-level local definition for `name` in `file`'s
/// semantic index. Returns `None` if the name doesn't appear (defensive,
/// shouldn't happen for a `Local` entry).
fn local_definition_range(file: File, db: &dyn Db, name: &str) -> Option<biome_rowan::TextRange> {
    file.semantic_index(db).file_exports().get(name).copied()
}
