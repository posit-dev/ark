use aether_syntax::AnyRExpression;
use aether_syntax::AnyRParameterName;
use aether_syntax::RBinaryExpression;
use aether_syntax::RParameter;
use aether_syntax::RSyntaxKind;
use aether_syntax::RSyntaxNode;
use biome_rowan::AstNode;
use biome_rowan::AstSeparatedList;
use biome_rowan::TextRange;
use oak_core::syntax_ext::RIdentifierExt;
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
    #[returns(copy)]
    pub file: File,
    /// The scope within `file` where the binding is introduced.
    #[returns(copy)]
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
        Some(name_node(self.kind(db), &root)?.text_trimmed_range())
    }

    /// Returns the formal parameters as written in the bound function's source.
    /// The value must be a function definition, either `function(...)` or
    /// `\(...)`. Other values, including `.Primitive()` calls, and bindings
    /// for parameters, loop variables or `source()` imports return `None`.
    ///
    /// Callers read one callee at a time, so this uses the memoized
    /// [`File::parse()`] tree without a separate tracked query.
    pub fn formals(self, db: &'db dyn crate::Db) -> Option<Vec<Formal>> {
        let root = self.file(db).parse(db).syntax();
        let AnyRExpression::RFunctionDefinition(function) = value_node(self.kind(db), &root)?
        else {
            return None;
        };
        let parameters = function.parameters().ok()?;

        let formals = parameters
            .items()
            .iter()
            .filter_map(|parameter| formal(&parameter.ok()?))
            .collect();
        Some(formals)
    }
}

/// A function parameter as written in its definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Formal {
    /// The parameter name without backticks, `...` for dots.
    pub name: String,
    /// Source text of the default expression, `None` without a default.
    pub default: Option<String>,
}

fn formal(parameter: &RParameter) -> Option<Formal> {
    let name = match parameter.name().ok()? {
        AnyRParameterName::RIdentifier(identifier) => identifier.name_text(),
        AnyRParameterName::RDots(dots) => dots.syntax().text_trimmed().to_string(),
        AnyRParameterName::RDotDotI(dot_dot_i) => dot_dot_i.syntax().text_trimmed().to_string(),
        AnyRParameterName::RBogusParameterName(_) => return None,
    };

    // While the user is typing `function(x = )`, keep the parameter even
    // though its default is missing and R would reject the definition.
    let default = parameter
        .default()
        .and_then(|default| default.value().ok())
        .map(|value| value.syntax().text_trimmed().to_string());

    Some(Formal { name, default })
}

/// The syntax node of the bound name for `kind`.
fn name_node(kind: &DefinitionKind, root: &RSyntaxNode) -> Option<RSyntaxNode> {
    let node = match kind {
        DefinitionKind::Assignment(ptr) | DefinitionKind::SuperAssignment(ptr) => {
            let node = ptr.to_node(root);
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
            let node = ptr.to_node(root);
            node.name().ok()?.into_syntax()
        },
        DefinitionKind::ForVariable(ptr) => {
            let node = ptr.to_node(root);
            node.variable().ok()?.into_syntax()
        },
        DefinitionKind::Assign { name, .. } => name.to_node(root).into_syntax(),
        // No name token at the binding site
        DefinitionKind::Import { .. } => return None,
    };
    Some(node)
}

/// The assigned value, accounting for assignment direction and `assign()`.
/// Return `None` for parameters, loop variables, imports, or a missing value.
pub(crate) fn value_node(kind: &DefinitionKind, root: &RSyntaxNode) -> Option<AnyRExpression> {
    match kind {
        DefinitionKind::Assignment(ptr) | DefinitionKind::SuperAssignment(ptr) => {
            let node = ptr.to_node(root);
            if is_right_assignment(&node) {
                node.left().ok()
            } else {
                node.right().ok()
            }
        },
        DefinitionKind::Assign { value, .. } => Some(value.as_ref()?.to_node(root)),
        DefinitionKind::Parameter(_) |
        DefinitionKind::ForVariable(_) |
        DefinitionKind::Import { .. } => None,
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
#[derive(Debug, PartialEq, Eq, salsa::SalsaValue)]
struct Definitions<'db> {
    by_site: FxHashMap<DefinitionSite, Definition<'db>>,
}

/// Map key for [`Definitions`]: a binding's `(scope, def_id)` site.
///
/// Mirrors ty's `DefinitionNodeKey`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::SalsaValue)]
struct DefinitionSite {
    scope_id: ScopeId,
    def_id: DefinitionId,
}
