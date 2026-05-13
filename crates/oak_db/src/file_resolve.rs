use biome_rowan::TextRange;

use crate::Db;
use crate::ExportEntry;
use crate::File;
use crate::Name;

/// The result of resolving a name to a concrete definition.
///
/// Carries `(File, name, range)`. The range is read from the resolved file's
/// `semantic_index`. Consumers (goto-def) can navigate directly to
/// `(file, range)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub file: File,
    pub name: String,
    pub range: TextRange,
}

#[salsa::tracked]
impl File {
    /// Resolve `name` against this file's exports.
    ///
    /// Chases `Import` forwarding entries (introduced by `source()`) through
    /// `exports(target_file)` until a `Local` is found. Returns `None` if the
    /// name isn't exported (or only chains through unresolved forwards /
    /// cycles, which `exports` recovers to empty).
    #[salsa::tracked]
    pub fn resolve(self, db: &dyn Db, name: Name<'_>) -> Option<Resolution> {
        let mut current_file = self;
        let mut current_name = name.text(db).to_string();

        loop {
            let entry = current_file.exports(db).get(&current_name)?.clone();
            match entry {
                ExportEntry::Local => {
                    let range = local_definition_range(current_file, db, &current_name)?;
                    return Some(Resolution {
                        file: current_file,
                        name: current_name,
                        range,
                    });
                },
                ExportEntry::Import { script, name } => {
                    current_file = script.file(db);
                    current_name = name;
                },
            }
        }
    }
}

/// Locate the range of a top-level local definition for `name` in `file`'s
/// semantic index. Returns `None` if the name doesn't appear (that's defensive,
/// shouldn't happen for a `Local` entry).
fn local_definition_range(file: File, db: &dyn Db, name: &str) -> Option<TextRange> {
    file.semantic_index(db).file_exports().get(name).copied()
}
