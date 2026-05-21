use std::collections::HashSet;
use std::rc::Rc;

use oak_semantic::semantic_index::ScopeId;

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
    /// or the bound expression read `def.kind(db)` and project per-variant.
    #[salsa::tracked]
    pub fn resolve(self, db: &'db dyn Db, name: Name<'db>) -> Option<Definition<'db>> {
        let mut current_file = self;
        let mut current_name: Rc<str> = Rc::from(name.text(db).as_str());

        // Defensive: cycle through `Import` is prevented upstream by
        // `exports()`'s `cycle_result` (which returns empty for one cycle
        // participant). The `Rc<str>` is cheap to clone (refcount bump).
        let mut visited: HashSet<(File, Rc<str>)> = HashSet::new();

        loop {
            if !visited.insert((current_file, current_name.clone())) {
                log::error!(
                    "Internal error: Cycle through `Import` forwards while resolving \
                    `{current_name}` in {url}.",
                    url = current_file.url(db),
                );
                return None;
            }

            match current_file.exports(db).get(current_name.as_ref())? {
                ExportEntry::Local => {
                    // Fetch exports again, this time through the semantic index
                    // to get the volatile `kind` field that the firewall query
                    // `File::exports()` doesn't expose.
                    let kind = current_file
                        .semantic_index(db)
                        .exports()
                        .get(current_name.as_ref())
                        .map(|def| def.kind().clone())?;

                    let file_scope = ScopeId::from(0);
                    return Some(Definition::new(
                        db,
                        current_file,
                        file_scope,
                        Name::new(db, current_name.as_ref()),
                        kind,
                    ));
                },

                ExportEntry::Import { file, name } => {
                    current_file = *file;
                    current_name = Rc::from(name.as_str());
                },
            }
        }
    }
}
