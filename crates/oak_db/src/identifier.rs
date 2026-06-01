use aether_syntax::AnyRSelector;
use aether_syntax::RExtractExpression;
use aether_syntax::RSyntaxKind;
use aether_syntax::RSyntaxNode;
use aether_syntax::RSyntaxToken;
use biome_rowan::AstNode;
use biome_rowan::TextRange;
use biome_rowan::TextSize;
use biome_rowan::TokenAtOffset;
use oak_core::syntax_ext::RIdentifierExt;
use oak_core::syntax_ext::RStringValueExt;

use crate::Db;
use crate::File;
use crate::Name;

/// The semantic identity of the identifier the cursor is on, after snapping
/// to the nearest name-token boundary.
#[derive(Debug, Clone)]
pub enum Identifier<'db> {
    /// Cursor on a binding use or definition site tracked by the semantic
    /// index. The range is the use or def site's text range. Its start is
    /// the offset to pass to `resolve_at`.
    Variable { name: Name<'db>, range: TextRange },
    /// Cursor on the RHS name of a `$` or `@` extract expression. Member
    /// names are not tracked by the semantic index.
    Member {
        name: String,
        kind: MemberKind,
        operator_range: TextRange,
        name_range: TextRange,
    },
}

/// The kind of `$` or `@` member-access operator.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MemberKind {
    Dollar,
    At,
}

impl<'db> Identifier<'db> {
    /// Classify the identifier at `offset` in `file`, snapping to the
    /// nearest name-token boundary first.
    ///
    /// Returns `None` when the cursor isn't on a variable binding/use or
    /// a member name (e.g. cursor is on an operator, a keyword, or a
    /// `pkg::sym` namespace access).
    pub fn classify(db: &'db dyn Db, file: File, offset: TextSize) -> Option<Identifier<'db>> {
        let parse = file.parse(db);
        let root = parse.syntax();
        let index = file.semantic_index(db);

        let snapped = snap_to_name_at_boundary(&root, offset);

        if let Some((scope_id, _use_id, use_site)) = index.use_at(snapped) {
            let name_str = index.symbols(scope_id).symbol(use_site.symbol()).name();
            return Some(Identifier::Variable {
                name: Name::new(db, name_str),
                range: use_site.range(),
            });
        }

        if let Some((scope_id, _def_id, def)) = index.definition_at(snapped) {
            let name_str = index.symbols(scope_id).symbol(def.symbol()).name();
            return Some(Identifier::Variable {
                name: Name::new(db, name_str),
                range: def.range(),
            });
        }

        if let Some((name, kind, operator_range, name_range)) = classify_member(&root, snapped) {
            return Some(Identifier::Member {
                name,
                kind,
                operator_range,
                name_range,
            });
        }

        None
    }
}

impl<'db> File {
    /// All use-site ranges for `name` in this file, across every scope.
    ///
    /// Used as the candidate pool for find-references: each returned range is
    /// confirmed by calling `resolve_at(range.start())` and checking whether
    /// its definition set intersects the target.
    pub fn uses_of(self, db: &'db dyn Db, name: Name<'db>) -> Vec<TextRange> {
        let index = self.semantic_index(db);
        let name_str = name.text(db);
        let mut ranges = Vec::new();

        for scope_id in index.scope_ids() {
            let symbols = index.symbols(scope_id);
            let Some(symbol_id) = symbols.id(name_str) else {
                continue;
            };
            for (_use_id, use_site) in index.uses(scope_id).iter() {
                if use_site.symbol() == symbol_id {
                    ranges.push(use_site.range());
                }
            }
        }

        ranges
    }

    /// All ranges where `name` appears as the RHS of a `$` or `@` with the
    /// given `kind` in this file. Structural scan of the parse tree.
    pub fn member_uses(self, db: &'db dyn Db, name: &str, kind: MemberKind) -> Vec<TextRange> {
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
                let rhs = extract.right().ok()?;
                let (member_name, range) = member_name_and_range(&rhs)?;
                if member_name != name {
                    return None;
                }
                Some(range)
            })
            .collect()
    }
}

/// Check whether the offset is on the RHS name of an `RExtractExpression`.
/// Returns `(name, kind, operator_range, name_range)` on a match.
fn classify_member(
    root: &RSyntaxNode,
    offset: TextSize,
) -> Option<(String, MemberKind, TextRange, TextRange)> {
    let token = root.token_at_offset(offset).right_biased()?;
    if !is_name_token(&token) {
        return None;
    }

    // For `foo$bar`: token `bar` -> parent `RIdentifier` -> parent `RExtractExpression`.
    let selector_node = token.parent()?;
    let extract = RExtractExpression::cast(selector_node.parent()?)?;

    // Token is after the operator -> it's on the RHS, not LHS.
    // RHS start == op end (adjacent bytes), so use strict <.
    let op = extract.operator().ok()?;
    if token.text_trimmed_range().start() < op.text_trimmed_range().end() {
        return None;
    }

    let kind = match op.kind() {
        RSyntaxKind::DOLLAR => MemberKind::Dollar,
        RSyntaxKind::AT => MemberKind::At,
        _ => return None,
    };

    let rhs = extract.right().ok()?;
    let (name, name_range) = member_name_and_range(&rhs)?;
    Some((name, kind, op.text_trimmed_range(), name_range))
}

fn member_name_and_range(selector: &AnyRSelector) -> Option<(String, TextRange)> {
    match selector {
        AnyRSelector::RIdentifier(ident) => {
            Some((ident.name_text(), ident.syntax().text_trimmed_range()))
        },
        AnyRSelector::RStringValue(s) => Some((s.string_text()?, s.syntax().text_trimmed_range())),
        _ => None,
    }
}

fn snap_to_name_at_boundary(root: &RSyntaxNode, offset: TextSize) -> TextSize {
    match root.token_at_offset(offset) {
        TokenAtOffset::None => offset,
        TokenAtOffset::Single(token) => {
            if is_name_token(&token) {
                token.text_trimmed_range().start()
            } else {
                offset
            }
        },
        TokenAtOffset::Between(left, right) => {
            if is_name_token(&left) {
                left.text_trimmed_range().start()
            } else if is_name_token(&right) {
                right.text_trimmed_range().start()
            } else {
                offset
            }
        },
    }
}

fn is_name_token(token: &RSyntaxToken) -> bool {
    matches!(
        token.kind(),
        RSyntaxKind::IDENT | RSyntaxKind::R_STRING_LITERAL
    )
}
