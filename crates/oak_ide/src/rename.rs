use aether_syntax::RSyntaxNode;
use anyhow::anyhow;
use biome_rowan::TextRange;
use oak_core::identifier::to_identifier_text;
use oak_semantic::semantic_index::SemanticIndex;

use crate::find_references;
use crate::FilePosition;
use crate::FileRange;
use crate::Identifier;

/// All edits needed to rename the symbol at the cursor. Each range gets
/// replaced by `new_text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameTargets {
    pub ranges: Vec<FileRange>,
    /// The canonical syntactic form of the new name (backtick-wrapped if
    /// needed). Same for every range in `ranges`.
    pub new_text: String,
}

/// Identify the renamable identifier at `position`, returning its range
/// and current (unquoted) name. Returns `None` when the cursor is on a
/// non-identifier, a `pkg::sym` namespace access, or a `$`/`@` member
/// name (TODO(places)).
///
/// Equivalent to LSP `textDocument/prepareRename`. Clients use the range
/// to highlight the editable region and the name as the placeholder.
pub fn prepare_rename(
    index: &SemanticIndex,
    root: &RSyntaxNode,
    position: &FilePosition,
) -> Option<(TextRange, String)> {
    let ident = Identifier::classify(index, root, position.offset)?;
    match ident {
        Identifier::Definition { def, name, .. } => Some((def.range(), name.to_string())),
        Identifier::Use { use_site, name, .. } => Some((use_site.range(), name.to_string())),
        Identifier::NamespaceAccess { .. } => None,
    }
}

/// Compute all rename edits within the file.
///
/// Returns `Err` when:
/// - `new_name` is empty, is an R reserved word, or contains a literal
///   backtick (which can't appear in a backtick-quoted identifier).
/// - The cursor isn't on a renamable identifier (no `prepare_rename`
///   target).
/// - Nothing in the file binds to the cursor's symbol (free variable
///   from outside the file). Rename would only edit local sites without
///   touching the external definition, so we refuse rather than produce
///   a partial result.
///
/// TODO(places): renaming a `$`/`@` member name returns `Err` because
/// the semantic index doesn't track member names.
///
/// TODO(salsa): cross-file renames are out of scope until cross-file
/// resolution lands. For now this is intra-file only.
pub fn rename(
    index: &SemanticIndex,
    root: &RSyntaxNode,
    position: &FilePosition,
    new_name: &str,
) -> anyhow::Result<RenameTargets> {
    let new_text = to_identifier_text(new_name)?;

    if prepare_rename(index, root, position).is_none() {
        return Err(anyhow!("No renamable identifier at cursor"));
    }

    let ranges = find_references(index, root, position, true);
    if ranges.is_empty() {
        return Err(anyhow!(
            "Cannot rename: symbol has no local binding in this file"
        ));
    }

    Ok(RenameTargets { ranges, new_text })
}
