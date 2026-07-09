use aether_syntax::RExtractExpression;
use aether_syntax::RNamespaceExpression;
use aether_syntax::RSyntaxKind;
use aether_syntax::RSyntaxNode;
use aether_syntax::RSyntaxToken;
use biome_rowan::AstNode;
use biome_rowan::TextRange;
use biome_rowan::TextSize;
use biome_rowan::TokenAtOffset;
use oak_core::syntax_ext::AnyRSelectorExt;

use crate::Db;
use crate::Definition;
use crate::File;
use crate::Name;

/// The semantic identity of the identifier the cursor is on, after snapping
/// to the nearest name-token boundary.
#[derive(Debug, Clone)]
pub enum Identifier<'db> {
    /// Cursor on a binding use or definition site tracked by the semantic
    /// index. The range is the use or def site's text range.
    Variable { name: Name<'db>, range: TextRange },
    /// Cursor on the RHS name of a `$` or `@` extract expression. Member
    /// names are not tracked by the semantic index.
    Member {
        name: Name<'db>,
        kind: MemberKind,
        operator_range: TextRange,
        name_range: TextRange,
    },
    /// Cursor anywhere on a `pkg::sym` or `pkg:::sym` namespace access. The
    /// whole qualified name is treated as one symbol, so the classification is
    /// the same wherever the cursor sits.
    NamespaceAccess {
        package: Name<'db>,
        name: Name<'db>,
        visibility: NamespaceVisibility,
    },
}

/// The kind of `$` or `@` member-access operator.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MemberKind {
    Dollar,
    At,
}

/// R's `::` vs `:::` distinction
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NamespaceVisibility {
    Exported,
    Internal,
}

impl<'db> Identifier<'db> {
    /// Classify the identifier at `offset` in `file`, snapping to the
    /// nearest name-token boundary first.
    ///
    /// Returns `None` when the cursor isn't on a variable binding/use, a
    /// member name, or a namespace access (e.g. cursor is on an operator or
    /// a keyword).
    pub fn classify(db: &'db dyn Db, file: File, offset: TextSize) -> Option<Identifier<'db>> {
        let parse = file.parse(db);
        let root = parse.syntax();
        let index = file.semantic_index(db);

        let token = snap_to_name_token(&root, offset)?;
        let offset = token.text_trimmed_range().start();

        if let Some((scope_id, _use_id, use_site)) = index.use_at(offset) {
            let name = index.symbols(scope_id).symbol(use_site.symbol()).name();
            return Some(Identifier::Variable {
                name: Name::new(db, name),
                range: use_site.range(),
            });
        }

        if let Some((scope_id, _def_id, def)) = index.definition_at(offset) {
            let name = index.symbols(scope_id).symbol(def.symbol()).name();
            return Some(Identifier::Variable {
                name: Name::new(db, name),
                range: def.range(),
            });
        }

        if let Some((name, kind, operator_range, name_range)) = classify_member(&token) {
            return Some(Identifier::Member {
                name: Name::new(db, name.as_str()),
                kind,
                operator_range,
                name_range,
            });
        }

        if let Some((package, name, visibility)) = classify_namespace(&token) {
            return Some(Identifier::NamespaceAccess {
                package: Name::new(db, package.as_str()),
                name: Name::new(db, name.as_str()),
                visibility,
            });
        }

        None
    }
}

impl<'db> File {
    /// The variable at `offset` and the definitions it resolves to.
    ///
    /// `None` when the cursor isn't on a `Variable` (a member name, a namespace
    /// access, or a non-name). The semantic index only tracks variables, so
    /// they're the only classification `resolve_at` can answer for.
    pub fn resolve_variable_at(
        self,
        db: &'db dyn Db,
        offset: TextSize,
    ) -> Option<(Name<'db>, TextRange, Vec<Definition<'db>>)> {
        let Some(Identifier::Variable { name, range }) = Identifier::classify(db, self, offset)
        else {
            return None;
        };
        let defs = self.resolve_at(db, range.start());
        Some((name, range, defs))
    }

    /// All use-site ranges for `name` in this file, across every scope.
    pub fn uses_of(self, db: &'db dyn Db, name: Name<'db>) -> Vec<TextRange> {
        self.semantic_index(db)
            .uses_of(name.text(db).as_str())
            .into_iter()
            .map(|(_scope_id, _use_id, use_site)| use_site.range())
            .collect()
    }

    /// All ranges where `name` appears as the RHS of a `$` or `@` with the
    /// given `kind` in this file. Structural scan of the parse tree.
    pub fn member_uses_of(self, db: &'db dyn Db, name: &str, kind: MemberKind) -> Vec<TextRange> {
        let root = self.parse(db).syntax();
        root.descendants()
            .filter_map(RExtractExpression::cast)
            .filter_map(|extract| {
                let op = extract.operator().ok()?;
                let op_kind = match op.kind() {
                    RSyntaxKind::DOLLAR => MemberKind::Dollar,
                    RSyntaxKind::AT => MemberKind::At,
                    _ => return None,
                };
                if op_kind != kind {
                    return None;
                }
                let right = extract.right().ok()?;
                let right_name = right.identifier_text()?;
                if right_name != name {
                    return None;
                }
                Some(right.syntax().text_trimmed_range())
            })
            .collect()
    }

    /// All ranges where `name` appears as the RHS symbol of a `::` or `:::`
    /// with `namespace` on the left, in this file. Structural scan of the
    /// parse tree. `::` and `:::` both count: they name the same symbol.
    pub fn namespace_uses_of(self, db: &'db dyn Db, namespace: &str, name: &str) -> Vec<TextRange> {
        let root = self.parse(db).syntax();
        root.descendants()
            .filter_map(RNamespaceExpression::cast)
            .filter_map(|namespace_expr| {
                let left = namespace_expr.left().ok()?;
                let left_name = left.identifier_text()?;
                if left_name != namespace {
                    return None;
                }
                let right = namespace_expr.right().ok()?;
                let right_name = right.identifier_text()?;
                if right_name != name {
                    return None;
                }
                Some(right.syntax().text_trimmed_range())
            })
            .collect()
    }
}

/// Check whether `token` is the RHS name of an `RExtractExpression`.
/// Returns `(name, kind, operator_range, name_range)` on a match.
fn classify_member(token: &RSyntaxToken) -> Option<(String, MemberKind, TextRange, TextRange)> {
    // For `foo$bar`: token `bar` -> parent `RIdentifier` -> parent `RExtractExpression`.
    let selector_node = token.parent()?;
    let extract = RExtractExpression::cast(selector_node.parent()?)?;

    // Token is after the operator -> it's on the RHS, not LHS
    let op = extract.operator().ok()?;
    if token.text_trimmed_range().start() < op.text_trimmed_range().end() {
        return None;
    }

    let kind = match op.kind() {
        RSyntaxKind::DOLLAR => MemberKind::Dollar,
        RSyntaxKind::AT => MemberKind::At,
        _ => return None,
    };

    let right = extract.right().ok()?;
    let name = right.identifier_text()?;
    let range = right.syntax().text_trimmed_range();

    Some((name, kind, op.text_trimmed_range(), range))
}

/// Check whether `token` is part of a `pkg::sym` / `pkg:::sym` namespace access. Returns
/// `(package, name, visibility)` on a match.
fn classify_namespace(token: &RSyntaxToken) -> Option<(String, String, NamespaceVisibility)> {
    // For `pkg::sym`: token -> parent `RIdentifier` -> parent `RNamespaceExpression`.
    // A cursor inside the `::` operator is already snapped onto the RHS.
    let identifier = token.parent()?;
    let expr = RNamespaceExpression::cast(identifier.parent()?)?;

    let left = expr.left().ok()?;
    let package: String = left.identifier_text()?;

    let right = expr.right().ok()?;
    let name = right.identifier_text()?;

    let op = expr.operator().ok()?;
    let visibility = match op.text_trimmed() {
        "::" => NamespaceVisibility::Exported,
        ":::" => NamespaceVisibility::Internal,
        _ => return None,
    };

    Some((package, name, visibility))
}

/// The name token the cursor is on, after snapping to the nearest name-token
/// boundary. `None` when the cursor isn't on (or beside) a name.
///
/// A cursor at a name's edge sits between two tokens, so we prefer the name on
/// either side. The downstream `classify_*` helpers then read the offset back
/// as `token.text_trimmed_range().start()`.
fn snap_to_name_token(root: &RSyntaxNode, offset: TextSize) -> Option<RSyntaxToken> {
    match root.token_at_offset(offset) {
        TokenAtOffset::None => None,
        TokenAtOffset::Single(token) => {
            if is_name_token(&token) {
                Some(token)
            } else if matches!(token.kind(), RSyntaxKind::COLON2 | RSyntaxKind::COLON3) {
                // Cursor strictly inside `::` / `:::` (only multi-char operators
                // have an interior). Snap onto the qualified symbol on its right.
                token.next_token().filter(is_name_token)
            } else {
                None
            }
        },
        TokenAtOffset::Between(left, right) => {
            if is_name_token(&left) {
                Some(left)
            } else if is_name_token(&right) {
                Some(right)
            } else {
                None
            }
        },
    }
}

fn is_name_token(token: &RSyntaxToken) -> bool {
    // FIXME: Right now we're too liberal with strings. We should only match
    // them when they stand for an identifier, e.g. in the LHS of `<-` or in
    // function call position.
    matches!(
        token.kind(),
        RSyntaxKind::IDENT | RSyntaxKind::R_STRING_LITERAL
    )
}
