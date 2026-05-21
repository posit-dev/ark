use aether_syntax::RBinaryExpression;
use aether_syntax::RSyntaxKind;
use biome_rowan::AstNode;
use biome_rowan::TextRange;
use oak_semantic::semantic_index::DefinitionKind;
use oak_semantic::semantic_index::ScopeId;

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
