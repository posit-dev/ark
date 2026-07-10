use aether_syntax::RBinaryExpression;
use aether_syntax::RSyntaxKind;
use biome_rowan::AstNode;
use biome_rowan::TextRange;
use oak_semantic::semantic_index::DefinitionId;
use oak_semantic::semantic_index::DefinitionKind;
use oak_semantic::semantic_index::ScopeId;
use rustc_hash::FxHashMap;

use crate::Db;
use crate::File;
use crate::Name;

/// A name binding identified by `(file, scope, name)`.
///
/// `Definition` is a salsa-tracked entity. Identity is determined by the
/// untracked fields, so the same `(file, scope, name)` tuple maps to the same
/// salsa id across revisions, even when the source position of the binding
/// shifts. This mirrors ty's `Definition<'db>` shape (see
/// `crates/ty_python_semantic/src/semantic_index/definition.rs`).
///
/// `kind` is the binding's [`DefinitionKind`] from `oak_semantic`. Marked
/// `#[tracked]` so it isn't part of the entity's identity: edits that change
/// the AST update `kind` in place while the salsa id (keyed on `file`, `scope`,
/// `name`) stays stable. Without `#[tracked]`, every AST change would produce a
/// new entity id and invalidate all consumers, including those that only care
/// about identity.
///
/// Also `no_eq` because the embedded `AstPtr`s shift on any edit moving the
/// binding's line, so backdating would never fire. The point isn't backdating,
/// it's per-field invalidation.
#[salsa::tracked(debug)]
pub struct Definition<'db> {
    /// The file containing the binding.
    pub file: File,
    /// The scope within `file` where the binding is introduced.
    pub scope: ScopeId,
    /// The interned name being bound. Stable across revisions for the
    /// same identifier text.
    pub name: Name<'db>,
    /// The classified binding expression containing both the identifier and
    /// definition.
    #[tracked]
    #[no_eq]
    pub kind: DefinitionKind,
}

impl<'db> Definition<'db> {
    /// Source range of the bound name (the `x` in `x <- 1`, the `"x"` in
    /// `"x" <- 1`, etc.). Returns `None` for `Import` kinds, which don't
    /// have a name token at the binding site (consumers follow the chain
    /// via `File::resolve`).
    pub fn name_range(self, db: &'db dyn crate::Db) -> Option<TextRange> {
        let parse = self.file(db).parse(db);
        let root = parse.tree().syntax().clone();

        let name_node = match self.kind(db) {
            DefinitionKind::Assignment(ptr) | DefinitionKind::SuperAssignment(ptr) => {
                let node = ptr.to_node(&root);
                // Right-assign (`rhs -> x`, `rhs ->> x`) puts the target on
                // the right, every other form (`x <- rhs`, `x <<- rhs`,
                // `x = rhs`) puts it on the left.
                let target = if is_right_assignment(&node) {
                    node.right().ok()?
                } else {
                    node.left().ok()?
                };
                target.into_syntax()
            },
            DefinitionKind::Parameter(ptr) => {
                let node = ptr.to_node(&root);
                node.name().ok()?.into_syntax()
            },
            DefinitionKind::ForVariable(ptr) => {
                let node = ptr.to_node(&root);
                node.variable().ok()?.into_syntax()
            },
            DefinitionKind::Import { .. } => return None,
            DefinitionKind::Assign { name, .. } => name.to_node(&root).into_syntax(),
        };
        Some(name_node.text_trimmed_range())
    }
}

fn is_right_assignment(node: &RBinaryExpression) -> bool {
    node.operator().is_ok_and(|op| {
        matches!(
            op.kind(),
            RSyntaxKind::ASSIGN_RIGHT | RSyntaxKind::SUPER_ASSIGN_RIGHT
        )
    })
}

#[salsa::tracked]
impl<'db> File {
    /// Look up the `Definition` entity for the binding at `(scope, def_id)` in
    /// this file. The entity has already been minted in [`File::definitions`]
    /// and this is a plain lookup. The salsa id stays stable across edits that
    /// renumber `def_id`.
    pub(crate) fn definition(
        self,
        db: &'db dyn Db,
        scope_id: ScopeId,
        def_id: DefinitionId,
    ) -> Option<Definition<'db>> {
        self.definitions(db)
            .by_site
            .get(&DefinitionSite { scope_id, def_id })
            .copied()
    }

    /// Mint every `Definition` in this file, keyed by its `(scope, def_id)`
    /// site. This is the single `Definition::new` call site: every resolution
    /// path looks entities up here rather than minting its own, so the same
    /// binding has one salsa id no matter how it's reached.
    ///
    /// Identity is `(file, scope, name)` (see [`Definition`]). `def_id` is only
    /// the lookup key, never part of identity, so inserting a binding earlier
    /// in a scope doesn't churn the ids of the others.
    #[salsa::tracked(returns(ref))]
    fn definitions(self, db: &'db dyn Db) -> Definitions<'db> {
        let index = self.semantic_index(db);
        let mut by_site = FxHashMap::default();

        for scope_id in index.scope_ids() {
            let symbols = index.symbols(scope_id);
            for (def_id, def) in index.definitions(scope_id).iter() {
                let name = Name::new(db, symbols.symbol(def.symbol()).name());
                let definition = Definition::new(db, self, scope_id, name, def.kind().clone());
                by_site.insert(DefinitionSite { scope_id, def_id }, definition);
            }
        }

        Definitions { by_site }
    }
}

/// Every `Definition` in a file, keyed by its definition site.
#[derive(Debug, PartialEq, Eq, salsa::Update)]
struct Definitions<'db> {
    by_site: FxHashMap<DefinitionSite, Definition<'db>>,
}

/// Map key for [`Definitions`]: a binding's `(scope, def_id)` site.
///
/// Mirrors ty's `DefinitionNodeKey`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
struct DefinitionSite {
    scope_id: ScopeId,
    def_id: DefinitionId,
}
